; RetroGit installer for Windows (Inno Setup 6): per-user, no administrator rights.
;   iscc installer\retrogit.iss          (after cargo build --release, from the repository)
; The version comes from RETROGIT_VERSION (set by the CI).

#define AppVersion GetEnv("RETROGIT_VERSION")
#if AppVersion == ""
  #define AppVersion "0.0.0"
#endif
#define Exe "..\target\x86_64-pc-windows-msvc\release\retrogit.exe"

[Setup]
; Never change: Windows recognizes RetroGit's installation by it (updates, uninstall).
AppId={{77B00662-D44A-4EBF-B0EB-88116047CC9B}
AppName=RetroGit
AppVersion={#AppVersion}
AppVerName=RetroGit {#AppVersion}
AppPublisher=RetroGit
AppPublisherURL=https://github.com/MadjidSahki/RetroGit
DefaultDirName={localappdata}\Programs\RetroGit
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=RetroGit-windows-x64-setup
SetupIconFile=..\crates\app\assets\RetroGit.ico
UninstallDisplayIcon={app}\retrogit.exe
UninstallDisplayName=RetroGit
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
CloseApplications=yes

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "{#Exe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD_PARTY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\crates\app\assets\icon-256.png"; DestDir: "{app}"; DestName: "icon.png"; Flags: ignoreversion

[Icons]
; The AppUserModelID ties notifications to this shortcut (name and icon).
Name: "{userprograms}\RetroGit"; Filename: "{app}\retrogit.exe"; AppUserModelID: "RetroGit"
Name: "{userdesktop}\RetroGit"; Filename: "{app}\retrogit.exe"; Tasks: desktopicon

[Registry]
; Same entries as RetroGit writes at start (notify/winreg.rs), removed on uninstall.
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\RetroGit"; ValueType: string; ValueName: "DisplayName"; ValueData: "RetroGit"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\RetroGit"; ValueType: string; ValueName: "IconUri"; ValueData: "{app}\icon.png"
Root: HKCU; Subkey: "Software\Classes\retrogit"; ValueType: string; ValueName: ""; ValueData: "URL:RetroGit"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\retrogit"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\retrogit\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\retrogit.exe"" ""%1"""

[Run]
Filename: "{app}\retrogit.exe"; Description: "{cm:LaunchProgram,RetroGit}"; Flags: nowait postinstall skipifsilent
; Updates from RetroGit run the installer silently with /RELAUNCH: start the new version.
Filename: "{app}\retrogit.exe"; Flags: nowait; Check: IsRelaunch

[Code]
function IsRelaunch: Boolean;
begin
  Result := Pos('/RELAUNCH', UpperCase(GetCmdTail)) > 0;
end;
