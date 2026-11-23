; PowerForge installer (Inno Setup 6). Per-user: no administrator rights needed.

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#ifndef BuildDir
  #define BuildDir "..\target\release"
#endif

[Setup]
AppId={{B3E61F2A-7C94-4D18-9A5E-2F0C8D41E7B6}
AppName=PowerForge
AppVersion={#AppVersion}
AppVerName=PowerForge {#AppVersion}
AppPublisher=Zheer Khalid
AppPublisherURL=https://zheer.dev
AppSupportURL=https://github.com/zheerkhalid/powerforge
AppUpdatesURL=https://github.com/zheerkhalid/powerforge/releases
DefaultDirName={autopf}\PowerForge
DefaultGroupName=PowerForge
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=PowerForge-Setup
SetupIconFile={#BuildDir}\powerforge.ico
UninstallDisplayIcon={app}\PowerForge.exe
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=no
LicenseFile=..\LICENSE
VersionInfoVersion={#AppVersion}
VersionInfoCompany=Zheer Khalid (zheer.dev)
VersionInfoDescription=PowerForge Setup

[Tasks]
Name: "startup"; Description: "Start PowerForge when I sign in to Windows (reminders, auto switch and keep-awake from the notification area)"; GroupDescription: "Background:"; Flags: unchecked
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BuildDir}\PowerForge.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\PowerForge"; Filename: "{app}\PowerForge.exe"
Name: "{group}\Uninstall PowerForge"; Filename: "{uninstallexe}"
Name: "{autodesktop}\PowerForge"; Filename: "{app}\PowerForge.exe"; Tasks: desktopicon

[Registry]
; The same value the "Start with Windows" switch in Settings writes.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "PowerForge"; ValueData: """{app}\PowerForge.exe"" --background"; Tasks: startup; Flags: uninsdeletevalue

[Run]
Filename: "{app}\PowerForge.exe"; Description: "{cm:LaunchProgram,PowerForge}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Ask PowerForge to release keep-awake, save and exit.
Filename: "{app}\PowerForge.exe"; Parameters: "--quit"; Flags: runhidden; RunOnceId: "QuitPowerForge"

[Code]
// Before files are replaced on an upgrade: ask a running PowerForge to exit.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
  Exe: String;
begin
  Result := '';
  Exe := ExpandConstant('{app}\PowerForge.exe');
  if FileExists(Exe) then
  begin
    Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    Sleep(1500);
  end;
end;

// The history in %LOCALAPPDATA%\PowerForge is the user's: it is kept on
// uninstall unless they say otherwise. The startup value is always removed.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep = usUninstall then
    Sleep(1500);
  if CurUninstallStep = usPostUninstall then
  begin
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'PowerForge');
    DataDir := ExpandConstant('{localappdata}\PowerForge');
    if DirExists(DataDir) and not UninstallSilent then
      if MsgBox('Also delete the battery history PowerForge recorded on this PC?' + #13#10 + DataDir + #13#10#13#10 +
                'Choose No to keep it.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(DataDir, True, True, True);
  end;
end;
