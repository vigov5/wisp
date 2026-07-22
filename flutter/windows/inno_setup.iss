#ifndef AppVersion
#define AppVersion "0.0.0"
#endif

[Setup]
AppId={{DE850239-5B22-480D-B91F-3413B6B98CC4}}
AppName=Wisp
AppVersion={#AppVersion}
AppPublisher=vigov5
DefaultDirName={autopf}\Wisp
DefaultGroupName=Wisp
OutputDir=.\
OutputBaseFilename=wisp-windows-setup
SetupIconFile=runner\resources\app_icon.ico
; Without this, Inno leaves the uninstall registry's DisplayIcon empty, so
; Windows Settings -> Installed apps (and Programs and Features) show a generic
; icon for Wisp. Point it at the installed exe so that list uses our real icon.
UninstallDisplayIcon={app}\Wisp.exe
Compression=lzma
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\build\windows\x64\runner\Release\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Wisp"; Filename: "{app}\Wisp.exe"
Name: "{commondesktop}\Wisp"; Filename: "{app}\Wisp.exe"; Tasks: desktopicon

[Run]
; postinstall shows a checked "Launch Wisp" box on the Finished page. The
; in-app auto-updater runs this installer with its wizard visible (not
; /SILENT), so that checkbox is what relaunches Wisp once the update lands.
Filename: "{app}\Wisp.exe"; Description: "{cm:LaunchProgram,Wisp}"; Flags: nowait postinstall
