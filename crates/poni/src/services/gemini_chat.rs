//! Worker asincrono per query rapide a Google Gemini (generativelanguage.googleapis.com).
//! Supporta più API Key separate da virgola/a capo con failover automatico su quota esaurita (429/403).

use crate::events::{AppEvent, EventSink, GeminiEvent};
use crate::services::http::{Client, Method, MtaGuard};
use std::time::Duration;

const USER_AGENT: &str = concat!("KDIStudio/", env!("CARGO_PKG_VERSION"));
const TIMEOUT: Duration = Duration::from_secs(45);

pub const MODELS: &[(&str, &str)] = &[
    ("Gemini 4 Hydrogen", "gemini-4-hydrogen"),
    ("Flash Lite 3.5", "gemini-3.5-flash-lite"),
    ("Gemini 3.8 Flash", "gemini-3.8-flash"),
    ("Gemini 4 Carbon", "gemini-4-carbon"),
];

pub fn load_api_keys() -> Vec<String> {
    let raw = crate::services::secret::load(&crate::paths::gemini_key_path())
        .or_else(|| std::env::var("GEMINI_API_KEY").ok())
        .unwrap_or_default();
    raw.split(|c| c == ',' || c == ';' || c == '\n' || c == '\r')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn load_api_key() -> Option<String> {
    load_api_keys().into_iter().next()
}

pub fn ask_async(prompt: String, model: &'static str, sink: EventSink) {
    let _ = std::thread::Builder::new().name("kdi-gemini".into()).spawn(move || {
        let _com = MtaGuard::new();
        let res = query_gemini(&prompt, model);
        sink.send(AppEvent::Gemini(GeminiEvent::Reply(res)));
    });
}

fn query_gemini(prompt: &str, model: &str) -> Result<String, String> {
    let keys = load_api_keys();
    if keys.is_empty() {
        return Err("Inserisci prima almeno una Gemini API Key.".to_string());
    }

    let client = Client::uncached(USER_AGENT).map_err(|e| format!("Errore rete: {e}"))?;
    let payload = serde_json::json!({
        "contents": [{
            "parts": [{"text": prompt}]
        }]
    })
    .to_string();

    let mut last_err = String::new();

    for (i, key) in keys.iter().enumerate() {
        let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={key}");
        let resp = match client.send(Method::Post, &url, &[], Some(&payload), TIMEOUT) {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("Richiesta fallita: {e}");
                continue;
            }
        };

        if (200..300).contains(&resp.status) {
            let val: serde_json::Value = serde_json::from_str(&resp.body)
                .map_err(|_| "Risposta JSON non valida da Gemini".to_string())?;
            let parts = val["candidates"][0]["content"]["parts"]
                .as_array()
                .ok_or_else(|| "Nessuna risposta generata da Gemini.".to_string())?;

            let mut out = String::new();
            for p in parts {
                if let Some(t) = p["text"].as_str() {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
            return if out.trim().is_empty() {
                Err("Risposta vuota da Gemini.".to_string())
            } else {
                Ok(out.trim().to_string())
            };
        }

        let err_msg = serde_json::from_str::<serde_json::Value>(&resp.body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("Errore HTTP {}", resp.status));

        // Se la chiave ha esaurito la quota/crediti (429 o 403) e ci sono altre chiavi, passa alla prossima
        let is_quota_or_auth = resp.status == 429 || resp.status == 403;
        if is_quota_or_auth && i + 1 < keys.len() {
            last_err = err_msg;
            continue;
        }
        return Err(err_msg);
    }

    Err(if last_err.is_empty() {
        "Tutte le API Key configurate hanno esaurito la quota.".to_string()
    } else {
        last_err
    })
}
