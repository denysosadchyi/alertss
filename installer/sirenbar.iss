; Inno Setup script: per-user installer for SirenBar (no admin rights needed).
; Build: ISCC installer\sirenbar.iss  ->  target\installer\sirenbar-setup.exe

#define AppName "SirenBar"
#define AppVersion "0.2.0"
#define AppExe "sirenbar.exe"

[Setup]
AppId={{B856CF85-C92C-4553-B072-2AB1EC9F9BA6}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Denys Osadchyi
AppPublisherURL=https://github.com/denysosadchyi/sirenbar
AppSupportURL=https://github.com/denysosadchyi/sirenbar/issues
DefaultDirName={localappdata}\SirenBar
DisableWelcomePage=no
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\target\installer
OutputBaseFilename=sirenbar-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern dynamic
WizardImageFile=img\wiz_100.png,img\wiz_125.png,img\wiz_150.png,img\wiz_200.png,img\wiz_250.png
WizardSmallImageFile=img\small_100.png,img\small_125.png,img\small_150.png,img\small_200.png,img\small_250.png
WizardImageStretch=no
SetupIconFile=..\assets\sirenbar.ico
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
UninstallDisplayIcon={app}\sirenbar.ico
UninstallDisplayName={#AppName} — повітряні тривоги

[Languages]
Name: "uk"; MessagesFile: "compiler:Languages\Ukrainian.isl"

[Messages]
WelcomeLabel1=SirenBar — повітряні тривоги на панелі задач
WelcomeLabel2=Невеликий віджет зліва на панелі задач показуватиме статус повітряної тривоги у вашій області:%n%n•  жовта — атака дронів%n•  червона — ракетна небезпека%n•  відбій — можна видихнути%n%nКлік по віджету відкриває карту України з усіма тривогами, райони під загрозою і журнал за день. Про початок і відбій тривоги прийде сповіщення Windows.%n%nПрава адміністратора не потрібні.
WizardSelectTasks=Налаштування
SelectTasksDesc=Як має працювати віджет?
SelectTasksLabel2=Виберіть потрібне й натисніть «Далі».
ReadyLabel1=Усе готово до встановлення SirenBar.
ReadyLabel2a=Натисніть «Встановити». Це займе кілька секунд.
FinishedHeadingLabel=SirenBar встановлено
FinishedLabelNoIcons=Віджет з'явиться зліва на панелі задач. Свою область виберіть кліком по карті у віджеті або через меню правого кліку.%n%nБережіть себе і не ігноруйте тривоги.
FinishedLabel=Віджет з'явиться зліва на панелі задач. Свою область виберіть кліком по карті у віджеті або через меню правого кліку.%n%nБережіть себе і не ігноруйте тривоги.
ConfirmUninstall=Видалити SirenBar? Віджет зникне з панелі задач і більше не попереджатиме про тривоги.

[Tasks]
Name: "autostart"; Description: "Запускати разом з Windows (рекомендовано — віджет завжди буде на панелі задач)"
Name: "startmenu"; Description: "Додати ярлик у меню «Пуск»"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\sirenbar.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\sirenbar.ico"; Comment: "Повітряні тривоги на панелі задач"; Tasks: startmenu

[Registry]
; Same value the app itself writes from its "Запускати з Windows" menu item.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "SirenBar"; ValueData: """{app}\{#AppExe}"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "Запустити SirenBar зараз"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM {#AppExe}"; Flags: runhidden; RunOnceId: "KillApp"

[Code]
// Stop a running copy so its exe can be replaced on update, and remove the app's
// previous incarnation "Alertss" (settings in %APPDATA%\alertss are migrated by the app).
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
  OldUninst: String;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM alertss.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  OldUninst := ExpandConstant('{localappdata}\alertss\unins000.exe');
  if FileExists(OldUninst) then
    Exec(OldUninst, '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(500);
  Result := '';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'SirenBar');
    RegDeleteKeyIncludingSubkeys(HKEY_CURRENT_USER, 'Software\Classes\AppUserModelId\SirenBar.Widget');
  end;
end;
