//! Client nativo LocalSend v2 su rete locale:
//! - Scansione parallela su tutte le sottoreti LAN (/24) porta 53317
//! - Handshake GET /api/localsend/v2/info e POST /api/localsend/v2/register (HTTPS/HTTP)
//! - Trasferimento file diretto (/api/localsend/v2/prepare-upload + /upload)

use crate::events::{AppEvent, EventSink};
use crate::services::http::MtaGuard;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{h, HSTRING};
use windows::Foundation::{AsyncOperationWithProgressCompletedHandler, AsyncStatus, IAsyncOperationWithProgress, Uri};
use windows::Security::Cryptography::Certificates::ChainValidationResult;
use windows::Storage::Streams::{DataReader, DataWriter, UnicodeEncoding};
use windows::Web::Http::Filters::HttpBaseProtocolFilter;
use windows::Web::Http::Headers::HttpMediaTypeHeaderValue;
use windows::Web::Http::{HttpBufferContent, HttpClient, HttpMethod, HttpRequestMessage, HttpStringContent};

const DEFAULT_PORT: u16 = 53317;
const ALIAS: &str = "KDI Studio";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSendDevice {
    pub fingerprint: String,
    pub alias: String,
    pub device_model: String,
    pub ip: Ipv4Addr,
    pub port: u16,
    pub protocol: String,
}

#[derive(Clone, Debug)]
pub enum LocalSendEvent {
    DevicesUpdated(Vec<LocalSendDevice>),
    Sending { alias: String, current: usize, total: usize },
    Sent { alias: String, count: usize },
    Error(String),
}

#[derive(Deserialize)]
struct DeviceInfoResp {
    alias: Option<String>,
    #[serde(rename = "deviceModel")]
    device_model: Option<String>,
    fingerprint: Option<String>,
    port: Option<u16>,
    protocol: Option<String>,
}

#[derive(Serialize)]
struct RegisterPayload<'a> {
    alias: &'a str,
    version: &'a str,
    #[serde(rename = "deviceModel")]
    device_model: &'a str,
    #[serde(rename = "deviceType")]
    device_type: &'a str,
    fingerprint: &'a str,
    port: u16,
    protocol: &'a str,
    download: bool,
}

#[derive(Serialize)]
struct FileMeta {
    id: String,
    #[serde(rename = "fileName")]
    file_name: String,
    size: u64,
    #[serde(rename = "fileType")]
    file_type: String,
}

#[derive(Serialize)]
struct PrepareUploadReq<'a> {
    info: RegisterPayload<'a>,
    files: HashMap<String, FileMeta>,
}

#[derive(Deserialize)]
struct PrepareUploadResp {
    #[serde(rename = "sessionId")]
    session_id: String,
    files: HashMap<String, String>,
}

pub fn discover_async(sink: EventSink) {
    let _ = std::thread::Builder::new().name("kdi-localsend-disc".into()).spawn(move || {
        let mut local_ips: Vec<Ipv4Addr> = super::phonedrop::lan_addresses()
            .into_iter()
            .filter_map(|ip| match ip {
                std::net::IpAddr::V4(v4) => Some(v4),
                _ => None,
            })
            .collect();

        if let Some(route_ip) = super::phonedrop::route_ipv4() {
            if !local_ips.contains(&route_ip) {
                local_ips.push(route_ip);
            }
        }

        if local_ips.is_empty() {
            sink.send(AppEvent::LocalSend(LocalSendEvent::Error("Nessuna rete Wi-Fi/LAN attiva.".into())));
            return;
        }

        // Deduplica le sottoreti /24 da scansionare
        let mut subnets: Vec<([u8; 3], Ipv4Addr)> = Vec::new();
        let mut seen_sub = HashSet::new();
        for ip in local_ips {
            let o = ip.octets();
            let prefix = [o[0], o[1], o[2]];
            if seen_sub.insert(prefix) {
                subnets.push((prefix, ip));
            }
        }

        let my_fp = format!("kdi-studio-{}", std::process::id());
        let found: Arc<Mutex<HashMap<String, LocalSendDevice>>> = Arc::new(Mutex::new(HashMap::new()));

        let mut handles = Vec::new();
        for (prefix, local_ip) in subnets {
            for chunk_idx in 0..16u8 {
                let found = found.clone();
                let sink = sink.clone();
                let my_fp = my_fp.clone();
                let h = std::thread::spawn(move || {
                    let _com = MtaGuard::new();
                    let mut client: Option<HttpClient> = None;
                    let start = chunk_idx as u16 * 16 + 1;
                    let end = (start + 15).min(254);

                    for host in start..=end {
                        let ip = Ipv4Addr::new(prefix[0], prefix[1], prefix[2], host as u8);
                        if ip == local_ip {
                            continue;
                        }
                        let addr = SocketAddr::V4(SocketAddrV4::new(ip, DEFAULT_PORT));
                        if TcpStream::connect_timeout(&addr, Duration::from_millis(450)).is_err() {
                            continue;
                        }

                        if client.is_none() {
                            client = create_insecure_http_client().ok();
                        }
                        let Some(c) = client.as_ref() else { continue };

                        if let Some(dev) = probe_device(c, ip, &my_fp) {
                            if let Ok(mut map) = found.lock() {
                                if map.insert(dev.fingerprint.clone(), dev).is_none() {
                                    let mut list: Vec<_> = map.values().cloned().collect();
                                    list.sort_by(|a, b| a.alias.cmp(&b.alias));
                                    sink.send(AppEvent::LocalSend(LocalSendEvent::DevicesUpdated(list)));
                                }
                            }
                        }
                    }
                });
                handles.push(h);
            }
        }

        for h in handles {
            let _ = h.join();
        }

        let mut list: Vec<_> = found.lock().map(|m| m.values().cloned().collect()).unwrap_or_default();
        list.sort_by(|a, b| a.alias.cmp(&b.alias));
        sink.send(AppEvent::LocalSend(LocalSendEvent::DevicesUpdated(list)));
    });
}

fn probe_device(client: &HttpClient, ip: Ipv4Addr, my_fp: &str) -> Option<LocalSendDevice> {
    let payload = serde_json::to_string(&RegisterPayload {
        alias: ALIAS,
        version: "2.1",
        device_model: "Windows",
        device_type: "desktop",
        fingerprint: my_fp,
        port: DEFAULT_PORT,
        protocol: "https",
        download: false,
    })
    .ok()?;

    for proto in ["https", "http"] {
        // 1. Prova GET /api/localsend/v2/info
        let info_url = format!("{proto}://{ip}:{DEFAULT_PORT}/api/localsend/v2/info");
        if let Ok((status, body)) = send_get(client, &info_url, Duration::from_secs(2)) {
            if (200..300).contains(&status) {
                if let Some(dev) = parse_device(&body, ip, proto, my_fp) {
                    return Some(dev);
                }
            }
        }
        // 2. Fallback POST /api/localsend/v2/register
        let reg_url = format!("{proto}://{ip}:{DEFAULT_PORT}/api/localsend/v2/register");
        if let Ok((status, body)) = send_json(client, &reg_url, &payload, Duration::from_secs(2)) {
            if (200..300).contains(&status) {
                if let Some(dev) = parse_device(&body, ip, proto, my_fp) {
                    return Some(dev);
                }
            }
        }
    }
    None
}

fn parse_device(body: &str, ip: Ipv4Addr, proto: &str, my_fp: &str) -> Option<LocalSendDevice> {
    let resp: DeviceInfoResp = serde_json::from_str(body).ok()?;
    let fp = resp.fingerprint.unwrap_or_else(|| ip.to_string());
    if fp == my_fp {
        return None;
    }
    Some(LocalSendDevice {
        fingerprint: fp,
        alias: resp.alias.unwrap_or_else(|| format!("LocalSend ({ip})")),
        device_model: resp.device_model.unwrap_or_default(),
        ip,
        port: resp.port.unwrap_or(DEFAULT_PORT),
        protocol: resp.protocol.unwrap_or_else(|| proto.to_string()),
    })
}

pub fn send_files_async(device: LocalSendDevice, paths: Vec<String>, sink: EventSink) {
    let _ = std::thread::Builder::new().name("kdi-localsend-send".into()).spawn(move || {
        let _com = MtaGuard::new();
        match send_files_sync(&device, &paths, &sink) {
            Ok(count) => sink.send(AppEvent::LocalSend(LocalSendEvent::Sent { alias: device.alias, count })),
            Err(err) => sink.send(AppEvent::LocalSend(LocalSendEvent::Error(err))),
        }
    });
}

fn send_files_sync(device: &LocalSendDevice, raw_paths: &[String], sink: &EventSink) -> Result<usize, String> {
    let mut files_to_send: Vec<(String, PathBuf, FileMeta)> = Vec::new();
    for raw in raw_paths {
        collect_files(Path::new(raw), &mut files_to_send);
    }
    if files_to_send.is_empty() {
        return Err("Nessun file valido da inviare.".into());
    }

    let client = create_insecure_http_client()?;
    let my_fp = format!("kdi-studio-{}", std::process::id());

    let mut meta_map = HashMap::new();
    for (id, _, meta) in &files_to_send {
        meta_map.insert(
            id.clone(),
            FileMeta {
                id: meta.id.clone(),
                file_name: meta.file_name.clone(),
                size: meta.size,
                file_type: meta.file_type.clone(),
            },
        );
    }

    let req_body = serde_json::to_string(&PrepareUploadReq {
        info: RegisterPayload {
            alias: ALIAS,
            version: "2.1",
            device_model: "Windows",
            device_type: "desktop",
            fingerprint: &my_fp,
            port: DEFAULT_PORT,
            protocol: "https",
            download: false,
        },
        files: meta_map,
    })
    .map_err(|e| format!("Errore JSON: {e}"))?;

    let base_url = format!("{}://{}:{}", device.protocol, device.ip, device.port);
    let prep_url = format!("{base_url}/api/localsend/v2/prepare-upload");

    sink.send(AppEvent::LocalSend(LocalSendEvent::Sending {
        alias: device.alias.clone(),
        current: 0,
        total: files_to_send.len(),
    }));

    let (status, body) = send_json(&client, &prep_url, &req_body, Duration::from_secs(45))?;
    if status == 403 {
        return Err(format!("{} ha rifiutato il trasferimento.", device.alias));
    }
    if !(200..300).contains(&status) {
        return Err(format!("Errore connessione ({status}) con {}.", device.alias));
    }

    let prep: PrepareUploadResp = serde_json::from_str(&body).map_err(|_| "Risposta non valida dal telefono.".to_string())?;
    let total = files_to_send.len();

    for (idx, (id, path, _)) in files_to_send.iter().enumerate() {
        let Some(token) = prep.files.get(id) else { continue };
        sink.send(AppEvent::LocalSend(LocalSendEvent::Sending {
            alias: device.alias.clone(),
            current: idx + 1,
            total,
        }));

        let bytes = std::fs::read(path).map_err(|e| format!("Errore lettura file: {e}"))?;
        let up_url = format!(
            "{base_url}/api/localsend/v2/upload?sessionId={}&fileId={}&token={}",
            url_encode(&prep.session_id),
            url_encode(id),
            url_encode(token)
        );
        let (up_status, _) = send_bytes(&client, &up_url, &bytes, Duration::from_secs(120))?;
        if !(200..300).contains(&up_status) {
            return Err(format!("Invio fallito sul file {} (HTTP {up_status}).", idx + 1));
        }
    }

    Ok(total)
}

fn collect_files(path: &Path, out: &mut Vec<(String, PathBuf, FileMeta)>) {
    if let Ok(md) = std::fs::metadata(path) {
        if md.is_file() {
            let id = format!("f{}", out.len() + 1);
            let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            let file_type = poni_core::phonedrop::http::content_type_for(&file_name).to_string();
            out.push((
                id.clone(),
                path.to_path_buf(),
                FileMeta { id, file_name, size: md.len(), file_type },
            ));
        } else if md.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    collect_files(&entry.path(), out);
                }
            }
        }
    }
}

fn create_insecure_http_client() -> Result<HttpClient, String> {
    let filter = HttpBaseProtocolFilter::new().map_err(|e| format!("Filtro HTTP: {e}"))?;
    let _ = filter.SetAllowAutoRedirect(false);
    let _ = filter.SetAllowUI(false);
    if let Ok(errors) = filter.IgnorableServerCertificateErrors() {
        let _ = errors.Append(ChainValidationResult::Untrusted);
        let _ = errors.Append(ChainValidationResult::Expired);
        let _ = errors.Append(ChainValidationResult::InvalidName);
        let _ = errors.Append(ChainValidationResult::IncompleteChain);
        let _ = errors.Append(ChainValidationResult::InvalidSignature);
        let _ = errors.Append(ChainValidationResult::WrongUsage);
        let _ = errors.Append(ChainValidationResult::RevocationFailure);
    }
    HttpClient::Create(&filter).map_err(|e| format!("HttpClient: {e}"))
}

fn send_get(client: &HttpClient, url: &str, timeout: Duration) -> Result<(u16, String), String> {
    let uri = Uri::CreateUri(&HSTRING::from(url)).map_err(|e| format!("URI: {e}"))?;
    let method = HttpMethod::Get().map_err(|e| format!("Method: {e}"))?;
    let req = HttpRequestMessage::Create(&method, &uri).map_err(|e| format!("Req: {e}"))?;

    let op = client.SendRequestAsync(&req).map_err(|e| format!("SendAsync: {e}"))?;
    let resp = wait_op(&op, Instant::now() + timeout)?;
    let status = resp.StatusCode().map_err(|e| format!("Status: {e}"))?.0 as u16;
    let body = read_resp_body(&resp, timeout)?;
    let _ = resp.Close();
    let _ = req.Close();
    Ok((status, body))
}

fn send_json(client: &HttpClient, url: &str, json: &str, timeout: Duration) -> Result<(u16, String), String> {
    let uri = Uri::CreateUri(&HSTRING::from(url)).map_err(|e| format!("URI: {e}"))?;
    let method = HttpMethod::Post().map_err(|e| format!("Method: {e}"))?;
    let req = HttpRequestMessage::Create(&method, &uri).map_err(|e| format!("Req: {e}"))?;
    let content = HttpStringContent::CreateFromStringWithEncodingAndMediaType(&HSTRING::from(json), UnicodeEncoding::Utf8, h!("application/json"))
        .map_err(|e| format!("Content: {e}"))?;
    req.SetContent(&content).map_err(|e| format!("SetContent: {e}"))?;

    let op = client.SendRequestAsync(&req).map_err(|e| format!("SendAsync: {e}"))?;
    let resp = wait_op(&op, Instant::now() + timeout)?;
    let status = resp.StatusCode().map_err(|e| format!("Status: {e}"))?.0 as u16;
    let body = read_resp_body(&resp, timeout)?;
    let _ = resp.Close();
    let _ = req.Close();
    Ok((status, body))
}

fn read_resp_body(resp: &windows::Web::Http::HttpResponseMessage, timeout: Duration) -> Result<String, String> {
    if let Ok(c) = resp.Content() {
        let buf = wait_op(&c.ReadAsBufferAsync().map_err(|e| format!("ReadBuf: {e}"))?, Instant::now() + timeout)?;
        let len = buf.Length().unwrap_or(0) as usize;
        if len > 0 {
            let reader = DataReader::FromBuffer(&buf).map_err(|e| format!("Reader: {e}"))?;
            let mut bytes = vec![0u8; len];
            reader.ReadBytes(&mut bytes).map_err(|e| format!("ReadBytes: {e}"))?;
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    Ok(String::new())
}

fn send_bytes(client: &HttpClient, url: &str, data: &[u8], timeout: Duration) -> Result<(u16, String), String> {
    let uri = Uri::CreateUri(&HSTRING::from(url)).map_err(|e| format!("URI: {e}"))?;
    let method = HttpMethod::Post().map_err(|e| format!("Method: {e}"))?;
    let req = HttpRequestMessage::Create(&method, &uri).map_err(|e| format!("Req: {e}"))?;

    let writer = DataWriter::new().map_err(|e| format!("Writer: {e}"))?;
    writer.WriteBytes(data).map_err(|e| format!("WriteBytes: {e}"))?;
    let buf = writer.DetachBuffer().map_err(|e| format!("Detach: {e}"))?;
    let content = HttpBufferContent::CreateFromBuffer(&buf).map_err(|e| format!("BufContent: {e}"))?;
    content
        .Headers()
        .and_then(|h| h.SetContentType(&HttpMediaTypeHeaderValue::Create(h!("application/octet-stream"))?))
        .map_err(|e| format!("Header: {e}"))?;
    req.SetContent(&content).map_err(|e| format!("SetContent: {e}"))?;

    let op = client.SendRequestAsync(&req).map_err(|e| format!("SendAsync: {e}"))?;
    let resp = wait_op(&op, Instant::now() + timeout)?;
    let status = resp.StatusCode().map_err(|e| format!("Status: {e}"))?.0 as u16;
    let _ = resp.Close();
    let _ = req.Close();
    Ok((status, String::new()))
}

fn wait_op<T, P>(op: &IAsyncOperationWithProgress<T, P>, deadline: Instant) -> Result<T, String>
where
    T: windows::core::RuntimeType + 'static,
    P: windows::core::RuntimeType + 'static,
{
    if op.Status().map_err(|e| format!("OpStatus: {e}"))? == AsyncStatus::Started {
        let (tx, rx) = mpsc::channel::<()>();
        let done = AsyncOperationWithProgressCompletedHandler::new(move |_, _| {
            let _ = tx.send(());
            Ok(())
        });
        op.SetCompleted(&done).map_err(|e| format!("SetCompleted: {e}"))?;
        let left = deadline.saturating_duration_since(Instant::now());
        if let Err(RecvTimeoutError::Timeout) = rx.recv_timeout(left) {
            let _ = op.Cancel();
            return Err("Timeout connessione con LocalSend.".into());
        }
    }
    op.GetResults().map_err(|e| format!("Errore rete LocalSend: {e}"))
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
