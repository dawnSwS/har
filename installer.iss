[Setup]
AppName=har CLI
AppVersion=1.1.0
DefaultDirName={userappdata}\har-cli
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=Output
OutputBaseFilename=har_installer
Compression=lzma
SolidCompression=yes
ChangesEnvironment=yes

[Files]
Source: "target\release\har.exe"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; 自动向当前用户的环境变量追加安装目录，实现全局命令行调用
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}'))

[Code]
// 检查避免重复写入环境变量
function NeedsAddPath(Param: string): boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;

// 卸载时自动擦除环境变量
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  OrigPath: string;
  Param: string;
begin
  if CurUninstallStep = usUninstall then
  begin
    Param := ExpandConstant('{app}');
    if RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath) then
    begin
      StringChangeEx(OrigPath, ';' + Param, '', True);
      StringChangeEx(OrigPath, Param + ';', '', True);
      StringChangeEx(OrigPath, Param, '', True);
      RegWriteStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath);
    end;
  end;
end;