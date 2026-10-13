; Inno Setup script: per-user installer for Alertss (no admin rights needed).
; Build: ISCC installer\alertss.iss  ->  target\installer\alertss-setup.exe

#define AppName "Alertss"
#define AppVersion "0.1.1"
#define AppExe "alertss.exe"

[Setup]
AppId={{6F1C2B7E-4A0D-4E8B-9C35-3D2A7F5E1B42}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Denys Osadchyi
AppPublisherURL=https://github.com/denysosadchyi/alertss
AppSupportURL=https://github.com/denysosadchyi/alertss/issues
DefaultDirName={localappdata}\alertss
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\target\installer
OutputBaseFilename=alertss-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName} — повітряні тривоги

[Languages]
Name: "uk"; MessagesFile: "compiler:Languages\Ukrainian.isl"

[Tasks]
Name: "autostart"; Description: "Запускати разом з Windows"
Name: "startmenu"; Description: "Ярлик у меню «Пуск»"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: startmenu

[Registry]
; Same value the app itself writes from its "Запускати з Windows" menu item.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Alertss"; ValueData: """{app}\{#AppExe}"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "Запустити {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM {#AppExe}"; Flags: runhidden; RunOnceId: "KillApp"

[Code]
// Stop a running copy so its exe can be replaced on update.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(500);
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Alertss');
end;
