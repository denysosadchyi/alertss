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
DisableWelcomePage=no
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\target\installer
OutputBaseFilename=alertss-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern dynamic
WizardImageFile=img\wiz_100.png,img\wiz_125.png,img\wiz_150.png,img\wiz_200.png,img\wiz_250.png
WizardSmallImageFile=img\small_100.png,img\small_125.png,img\small_150.png,img\small_200.png,img\small_250.png
WizardImageStretch=no
SetupIconFile=..\assets\alertss.ico
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
UninstallDisplayIcon={app}\alertss.ico
UninstallDisplayName={#AppName} — повітряні тривоги

[Languages]
Name: "uk"; MessagesFile: "compiler:Languages\Ukrainian.isl"

[Messages]
WelcomeLabel1=Alertss — повітряні тривоги на панелі задач
WelcomeLabel2=Невеликий віджет зліва на панелі задач показуватиме статус повітряної тривоги у вашій області:%n%n•  жовта — атака дронів%n•  червона — ракетна небезпека%n•  відбій — можна видихнути%n%nКлік по віджету відкриває карту України з усіма тривогами, райони під загрозою і журнал за день. Про початок і відбій тривоги прийде сповіщення Windows.%n%nПрава адміністратора не потрібні.
WizardSelectTasks=Налаштування
SelectTasksDesc=Як має працювати віджет?
SelectTasksLabel2=Виберіть потрібне й натисніть «Далі».
ReadyLabel1=Усе готово до встановлення Alertss.
ReadyLabel2a=Натисніть «Встановити». Це займе кілька секунд.
FinishedHeadingLabel=Alertss встановлено
FinishedLabelNoIcons=Віджет з'явиться зліва на панелі задач. Свою область виберіть кліком по карті у віджеті або через меню правого кліку.%n%nБережіть себе і не ігноруйте тривоги.
FinishedLabel=Віджет з'явиться зліва на панелі задач. Свою область виберіть кліком по карті у віджеті або через меню правого кліку.%n%nБережіть себе і не ігноруйте тривоги.
ConfirmUninstall=Видалити Alertss? Віджет зникне з панелі задач і більше не попереджатиме про тривоги.

[Tasks]
Name: "autostart"; Description: "Запускати разом з Windows (рекомендовано — віджет завжди буде на панелі задач)"
Name: "startmenu"; Description: "Додати ярлик у меню «Пуск»"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\alertss.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\alertss.ico"; Comment: "Повітряні тривоги на панелі задач"; Tasks: startmenu

[Registry]
; Same value the app itself writes from its "Запускати з Windows" menu item.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Alertss"; ValueData: """{app}\{#AppExe}"""; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "Запустити Alertss зараз"; Flags: nowait postinstall skipifsilent

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
