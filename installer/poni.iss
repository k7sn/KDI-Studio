; KDIStudios Installer (Inno Setup)

#ifndef AppVersion
  #define AppVersion "1.8.0"
#endif
#ifndef ExeSource
  #define ExeSource "..\target\release\poni.exe"
#endif
#define AppName "KDIStudios"
#define AppDir "Poni"
#define AppPublisher "KDIStudios"
#define AppExe "poni.exe"
#define IslandClass "Poni.Island"

[Setup]
AppId={{6F1D2B7A-3C84-4E59-A1D6-8B2E5F0C9A47}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppCopyright=Copyright (C) 2026 {#AppPublisher}
VersionInfoVersion={#AppVersion}
VersionInfoCompany={#AppPublisher}
VersionInfoDescription={#AppName} Setup
VersionInfoProductName={#AppName}
PrivilegesRequired=admin
DefaultDirName={localappdata}\Programs\{#AppDir}
DisableDirPage=yes
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=KDIStudios-Setup
SetupIconFile=..\crates\poni\res\poni.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Avvia automaticamente con Windows"; GroupDescription: "Avvio:"

[Files]
Source: "{#ExeSource}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "KDIStudios Dynamic Island"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#AppDir}"; ValueData: """{app}\{#AppExe}"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
; Apre automaticamente la porta 53530 su Windows Firewall per "Invia al telefono"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""KDIStudios Phone Drop"""; Flags: runhidden waituntilterminated
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""KDIStudios Phone Drop"" dir=in action=allow program=""{app}\{#AppExe}"" profile=any"; Flags: runhidden waituntilterminated
Filename: "{app}\{#AppExe}"; Description: "Avvia {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM {#AppExe} /F"; Flags: runhidden waituntilterminated; RunOnceId: "StopPoni"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""KDIStudios Phone Drop"""; Flags: runhidden waituntilterminated; RunOnceId: "RemoveFirewallRule"

[UninstallDelete]
Type: filesandordirs; Name: "{localappdata}\{#AppDir}"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  WM_CLOSE = $0010;

procedure StopPoni();
var
  I, ResultCode: Integer;
begin
  if FindWindowByClassName('{#IslandClass}') = 0 then
    Exit;
  PostMessage(FindWindowByClassName('{#IslandClass}'), WM_CLOSE, 0, 0);
  for I := 1 to 30 do
  begin
    if FindWindowByClassName('{#IslandClass}') = 0 then
      Exit;
    Sleep(100);
  end;
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM {#AppExe} /F', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopPoni();
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and not WizardIsTaskSelected('autostart') then
    RegDeleteValue(HKEY_CURRENT_USER, RunKey, '{#AppDir}');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    StopPoni();
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKEY_CURRENT_USER, RunKey, '{#AppDir}');
end;
