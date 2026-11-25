[Setup]
AppId={{8F6A3C2E-4D1B-4E9A-9C5D-7A2F1B8E3D4C}}
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
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{code:BuildUpdatedPath}"; Check: NeedsAddPath(ExpandConstant('{app}'))

[Code]
function NormalizePathEntry(const Entry: string): string;
begin
  Result := RemoveBackslashUnlessRoot(Trim(Entry));
end;

function PathContains(const PathList, Target: string): boolean;
var
  NormalizedTarget: string;
  Remaining, Item: string;
  SemiPos: Integer;
begin
  NormalizedTarget := Uppercase(NormalizePathEntry(Target));
  Remaining := PathList;
  Result := False;
  while Length(Remaining) > 0 do
  begin
    SemiPos := Pos(';', Remaining);
    if SemiPos > 0 then
    begin
      Item := Copy(Remaining, 1, SemiPos - 1);
      Delete(Remaining, 1, SemiPos);
    end
    else
    begin
      Item := Remaining;
      Remaining := '';
    end;
    if Uppercase(NormalizePathEntry(Item)) = NormalizedTarget then
    begin
      Result := True;
      exit;
    end;
  end;
end;

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
  Result := not PathContains(OrigPath, Param);
end;

// 构造追加后的 Path，避免空值前导分号或重复分号
function BuildUpdatedPath(Param: string): string;
var
  OrigPath, Target: string;
begin
  Target := ExpandConstant('{app}');
  if (not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath)) or (Trim(OrigPath) = '') then
  begin
    Result := Target;
    exit;
  end;
  if OrigPath[Length(OrigPath)] = ';' then
    Result := OrigPath + Target
  else
    Result := OrigPath + ';' + Target;
end;

// 卸载时按分号精确移除环境变量，并保持 REG_EXPAND_SZ 类型
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  OrigPath, NewPath, Remaining, Item: string;
  NormalizedTarget: string;
  SemiPos: Integer;
begin
  if CurUninstallStep = usUninstall then
  begin
    NormalizedTarget := Uppercase(NormalizePathEntry(ExpandConstant('{app}')));
    if RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', OrigPath) then
    begin
      Remaining := OrigPath;
      NewPath := '';
      while Length(Remaining) > 0 do
      begin
        SemiPos := Pos(';', Remaining);
        if SemiPos > 0 then
        begin
          Item := Copy(Remaining, 1, SemiPos - 1);
          Delete(Remaining, 1, SemiPos);
        end
        else
        begin
          Item := Remaining;
          Remaining := '';
        end;
        if (Trim(Item) <> '') and (Uppercase(NormalizePathEntry(Item)) <> NormalizedTarget) then
        begin
          if NewPath <> '' then
            NewPath := NewPath + ';' + Item
          else
            NewPath := Item;
        end;
      end;
      RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', NewPath);
    end;
  end;
end;