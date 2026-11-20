; MediaTranscriber 설치 프로그램 (Inno Setup 6)
;
; 빌드: scripts\build-release.ps1 (cargo release 빌드 후 ISCC 실행)
; 직접:  ISCC.exe /DMyAppVersion=0.2.0 installer\MediaTranscriber.iss
;
; - 기본은 현재 사용자용 설치 (관리자 권한 불필요, %LOCALAPPDATA%\Programs\MediaTranscriber)
; - FFmpeg 와 Whisper 모델은 포함하지 않는다. 앱이 처음 실행될 때 필요 시 내려받는다.
; - 제거 시 사용자 데이터(모델·FFmpeg·로그·설정·저장된 API Key)를 지울지 묻는다.

#ifndef MyAppVersion
  #define MyAppVersion "0.2.0"
#endif
#define MyAppName "MediaTranscriber"
#define MyAppExeName "MediaTranscriber.exe"

[Setup]
AppId={{EC7A28F5-91D7-415C-913A-DB135E456D5E}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
VersionInfoVersion={#MyAppVersion}
VersionInfoProductName={#MyAppName}
VersionInfoDescription={#MyAppName} 설치 프로그램
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename={#MyAppName}-Setup-{#MyAppVersion}
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Code]
const
  DataQuestion =
    '사용자 데이터도 삭제할까요?' + #13#10#13#10 +
    '- 내려받은 Whisper 모델과 FFmpeg' + #13#10 +
    '- 설정, 로그' + #13#10 +
    '- Windows 자격 증명 관리자에 저장한 API Key' + #13#10#13#10 +
    '변환 결과 TXT 파일은 삭제하지 않습니다.';

procedure DeleteUserData();
var
  ResultCode: Integer;
begin
  // directories 크레이트 경로: %LOCALAPPDATA%\MediaTranscriber, %APPDATA%\MediaTranscriber
  DelTree(ExpandConstant('{localappdata}\{#MyAppName}'), True, True, True);
  DelTree(ExpandConstant('{userappdata}\{#MyAppName}'), True, True, True);
  // 앱이 '이 PC에 기억' 을 선택했을 때만 저장한 키 (없으면 아무 일도 하지 않음)
  Exec(ExpandConstant('{sys}\cmdkey.exe'), '/delete:"MediaTranscriber/OpenAI API Key"', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Exec(ExpandConstant('{sys}\cmdkey.exe'), '/delete:"MediaTranscriber/Gemini API Key"', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    // 자동(무인) 제거에서는 사용자 데이터를 지우지 않는다.
    if not UninstallSilent() then
      if MsgBox(DataQuestion, mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DeleteUserData();
  end;
end;
