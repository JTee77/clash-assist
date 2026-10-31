#requires -Version 7
<#
.SYNOPSIS
  扫描 tauri 生成的 installer.nsi（及其 include 的 hooks.nsh）：除 $INSTDIR 外不得有 `RmDir /r`（Ellie PR55 r2 / r3）。

.DESCRIPTION
  NSIS 3.10 的 `RmDir /r` 会跟随 junction（scripts/test-nsis-hooks.ps1 实测）。M4b 起 Tauri 模板「删除应用数据」
  里原本的
      RmDir /r "$APPDATA\${BUNDLEID}"
      RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
  已换成 hooks.nsh 的 `!insertmacro CA_DELETE_TAURI_APP_DATA`（自带模板 src-tauri/windows/installer.nsi）。

  判定：去掉注释、拼接以 `\` 结尾的续行后，凡是 RMDir 指令（不分大小写，允许前面带标签 `lbl:`）的开关里有 `/r`：
    - 路径引用了 APPDATA / LOCALAPPDATA（$APPDATA、${LOCALAPPDATA}、$%APPDATA%、任何含 `AppData` 字样的路径）
      → 应用数据命中；
    - 路径以 $INSTDIR 开头（且不含 `..`）→ 只列出，不判失败；
    - 其他（含经变量传入的路径，如 `RmDir /r $0`）→ 同样算命中（r3：看不出删的是什么，就不放行）。
  命中时：-Strict（tag 构建）报 error、退出 1；否则（演练）报 warning、退出 0。
  正向对照（r3）：installer.nsi 必须 include 我们的 windows\hooks.nsh，并含 `!insertmacro CA_DELETE_TAURI_APP_DATA`
  与 `!insertmacro NSIS_HOOK_POSTUNINSTALL`，证明扫的是用自带模板真正生成的那份；缺任一项无论是否 strict 都退出 1。
  -Include 列出的文件（如 hooks.nsh）一起按同样规则扫描；宏和 !include 不展开，所以 hooks.nsh 要单独传入。

  用法：
    pwsh scripts/scan-nsis-rmdir.ps1 -Nsi <installer.nsi> [-Include <hooks.nsh>[,…]] [-Strict]
    pwsh scripts/scan-nsis-rmdir.ps1 -SelfTest
  退出码：0 = 通过（或非 strict 下只有 warning）；1 = strict 下命中，或正向对照不成立；2 = 文件不存在或读不了。
#>
[CmdletBinding(DefaultParameterSetName = 'Scan')]
param(
  [Parameter(Mandatory, ParameterSetName = 'Scan')] [string] $Nsi,
  [Parameter(ParameterSetName = 'Scan')] [string[]] $Include = @(),
  [Parameter(ParameterSetName = 'Scan')] [switch] $Strict,
  [Parameter(Mandatory, ParameterSetName = 'SelfTest')] [switch] $SelfTest
)
$ErrorActionPreference = 'Stop'

$AppDataRef = '(?i)\$\{?(LOCAL)?APPDATA\}?(?![A-Za-z0-9_])|\$%(LOCAL)?APPDATA%|AppData'
$InstDirRef = '(?i)^"?\$\{?INSTDIR\}?(?:\\|"|$)'
# 正向对照：生成的 installer.nsi 里必须有的指令（去掉注释、首尾空白后整行匹配）
$Required = @(
  @{ What = '!include 我们的 windows\hooks.nsh'; Pattern = '(?i)^!include\s+"[^"]*[\\/]windows[\\/]hooks\.nsh"$' }
  @{ What = '!insertmacro CA_DELETE_TAURI_APP_DATA（自带模板）'; Pattern = '^!insertmacro\s+CA_DELETE_TAURI_APP_DATA$' }
  @{ What = '!insertmacro NSIS_HOOK_POSTUNINSTALL'; Pattern = '^!insertmacro\s+NSIS_HOOK_POSTUNINSTALL$' }
)

# 去注释、拼续行后的逻辑行：@{ No; Text }
function Get-LogicalLines([string] $text) {
  $lines = $text -split '\r?\n'
  $logical = [Collections.Generic.List[object]]::new()
  $buf = ''; $start = 0
  for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($buf -eq '') { $start = $i + 1 }
    $l = $lines[$i]
    if ($l -match '\\\s*$') { $buf += ($l -replace '\\\s*$', ' '); continue }
    $logical.Add(@{ No = $start; Text = $buf + $l }); $buf = ''
  }
  if ($buf) { $logical.Add(@{ No = $start; Text = $buf }) }
  $out = [Collections.Generic.List[object]]::new()
  $inBlock = $false
  foreach ($ln in $logical) {
    $t = $ln.Text
    # 块注释 /* ... */（不处理同一行里多段的情况，模板里没有）
    if ($inBlock) { if ($t -match '\*/') { $t = $t.Substring($t.IndexOf('*/') + 2); $inBlock = $false } else { continue } }
    if ($t -match '/\*') {
      $head = $t.Substring(0, $t.IndexOf('/*'))
      if ($t.Substring($t.IndexOf('/*')) -notmatch '\*/') { $inBlock = $true }
      $t = $head
    }
    $t = $t.Trim()
    if ($t -eq '' -or $t.StartsWith(';') -or $t.StartsWith('#')) { continue }
    $out.Add(@{ No = $ln.No; Text = $t })
  }
  $out
}

# 返回 @{ Hits = 应用数据命中; Unknown = 其他（非 $INSTDIR）命中; InstDir = $INSTDIR 下的 }
function Find-RecursiveRmDir([string] $text, [string] $file = '') {
  $hits = @(); $unknown = @(); $inst = @()
  foreach ($ln in (Get-LogicalLines $text)) {
    $t = $ln.Text
    # 可选的标签前缀（`lbl: RMDir …`）
    if ($t -notmatch '^(?i)(?:[A-Za-z_.][\w.]*:\s*)?RMDir\b(?<rest>.*)$') { continue }
    $rest = $Matches.rest.Trim()
    $switches = [regex]::Matches($rest, '(?<=^|\s)/[A-Za-z]+') | ForEach-Object { $_.Value.ToLowerInvariant() }
    if ($switches -notcontains '/r') { continue }
    $path = ($rest -replace '(?<=^|\s)/[A-Za-z]+', '').Trim()
    $entry = "${file}第 $($ln.No) 行：$t"
    if ($rest -match $AppDataRef) { $hits += $entry }
    elseif ($path -match $InstDirRef -and $path -notmatch '\.\.') { $inst += $entry }
    else { $unknown += $entry }
  }
  @{ Hits = $hits; Unknown = $unknown; InstDir = $inst }
}

function Find-MissingRequired([string] $text) {
  $lines = @(Get-LogicalLines $text | ForEach-Object { $_.Text })
  @($Required | Where-Object { $p = $_.Pattern; -not ($lines | Where-Object { $_ -match $p }) } | ForEach-Object { $_.What })
}

if ($SelfTest) {
  $ours = "!include `"D:\a\clash-assist\src-tauri\windows\hooks.nsh`"`n!insertmacro CA_DELETE_TAURI_APP_DATA`n!insertmacro NSIS_HOOK_POSTUNINSTALL"
  $cases = @(
    @{ Name = 'Tauri 模板原样'; Text = "  SetShellVarContext current`n  RmDir /r `"`$APPDATA\`${BUNDLEID}`"`n  RmDir /r `"`$LOCALAPPDATA\`${BUNDLEID}`""; Hits = 2 }
    @{ Name = '小写与开关顺序'; Text = "rmdir /REBOOTOK /r `$LocalAppData\x"; Hits = 1 }
    @{ Name = '花括号写法'; Text = "RMDir /r `"`${LOCALAPPDATA}\x`""; Hits = 1 }
    @{ Name = '环境变量写法'; Text = "RMDir /r `"`$%APPDATA%\x`""; Hits = 1 }
    @{ Name = 'PROFILE\AppData'; Text = "RMDir /r `"`$PROFILE\AppData\Local\x`""; Hits = 1 }
    @{ Name = '续行'; Text = "RMDir /r \`n  `"`$APPDATA\x`""; Hits = 1 }
    @{ Name = '带标签（r3）'; Text = "lbl_x: RMDir /r `"`$APPDATA\x`"`nother.lbl:RmDir /r `$R1"; Hits = 1; Unknown = 1 }
    @{ Name = '经变量传入的路径算命中（r3）'; Text = "StrCpy `$0 `"`$APPDATA\x`"`nRmDir /r `$0`nRmDir /r `"`$R0\sub`""; Hits = 0; Unknown = 2 }
    @{ Name = '不递归的 RMDir'; Text = "RMDir `"`$APPDATA\x`"`nRMDir /REBOOTOK `"`$LOCALAPPDATA\x`""; Hits = 0; Unknown = 0 }
    @{ Name = '注释掉的'; Text = "; RmDir /r `"`$APPDATA\x`"`n# RmDir /r `$LOCALAPPDATA`n/* RmDir /r `$APPDATA`n*/"; Hits = 0; Unknown = 0 }
    @{ Name = 'INSTDIR 只列出'; Text = "RMDir /r `"`$INSTDIR\resources`"`nRMDir /r /REBOOTOK `$INSTDIR"; Hits = 0; Unknown = 0; InstDir = 2 }
    @{ Name = 'INSTDIR\.. 不放行'; Text = "RMDir /r `"`$INSTDIR\..\x`""; Hits = 0; Unknown = 1 }
    @{ Name = '含 AppData 字样一律算（宁严勿松）'; Text = "RMDir /r `"`$APPDATAX\x`""; Hits = 1 }
    @{ Name = '我们的 hooks（不用 /r）'; Text = "Push `"`$LOCALAPPDATA\ClashAssist`"`nCall un.CA_RmTreeNoFollow`nRMDir `"`$R0`""; Hits = 0; Unknown = 0 }
    @{ Name = '正向对照齐全'; Text = $ours; Missing = 0 }
    @{ Name = '正向对照：没 include hooks'; Text = ($ours -replace '(?m)^!include.*$', ''); Missing = 1 }
    @{ Name = '正向对照：没用自带模板'; Text = ($ours -replace '(?m)^!insertmacro CA_DELETE.*$', "RmDir /r `"`$APPDATA\x`""); Missing = 1; Hits = 1 }
    @{ Name = '正向对照：注释掉的不算'; Text = "; " + ($ours -replace "`n", "`n; "); Missing = 3 }
  )
  $fail = 0
  foreach ($c in $cases) {
    $r = Find-RecursiveRmDir $c.Text
    $m = Find-MissingRequired $c.Text
    $ok = $true
    foreach ($k in 'Hits', 'Unknown', 'InstDir') { if ($c.ContainsKey($k) -and $r[$k].Count -ne $c[$k]) { $ok = $false } }
    if ($c.ContainsKey('Missing') -and $m.Count -ne $c.Missing) { $ok = $false }
    if ($ok) { Write-Host "ok   $($c.Name)" }
    else { Write-Host "FAIL $($c.Name)：应用数据 $($r.Hits.Count)，其他 $($r.Unknown.Count)，INSTDIR $($r.InstDir.Count)，缺 $($m.Count)"; $fail = 1 }
  }
  Write-Host ('scan-nsis-rmdir 自测：' + ($(if ($fail) { '失败' } else { '全部通过' })))
  exit $fail
}

$files = @($Nsi) + $Include
foreach ($f in $files) { if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { Write-Host "::error::找不到 $f"; exit 2 } }
$level = if ($Strict) { 'error' } else { 'warning' }
$bad = 0
foreach ($f in $files) {
  $r = Find-RecursiveRmDir (Get-Content -Raw -LiteralPath $f) "$(Split-Path -Leaf $f) "
  foreach ($o in $r.InstDir) { Write-Host "  `$INSTDIR 下的 RmDir /r（不判失败）$o" }
  foreach ($h in $r.Hits) { Write-Host "::${level}::RmDir /r 删除应用数据目录（会跟随 junction）：$h"; $bad++ }
  foreach ($h in $r.Unknown) { Write-Host "::${level}::RmDir /r 的路径不是 `$INSTDIR（会跟随 junction）：$h"; $bad++ }
}
Write-Host "scan-nsis-rmdir：扫描 $($files -join '、')，除 `$INSTDIR 外的 RmDir /r 共 $bad 处"
$missing = Find-MissingRequired (Get-Content -Raw -LiteralPath $Nsi)
foreach ($m in $missing) { Write-Host "::error::$Nsi 里没有 $m：扫描的不是用自带模板和 hooks.nsh 生成的 installer.nsi" }
if ($missing.Count) { exit 1 }
Write-Host 'scan-nsis-rmdir：正向对照通过（含 hooks.nsh、CA_DELETE_TAURI_APP_DATA、NSIS_HOOK_POSTUNINSTALL）'
if ($bad -and $Strict) { exit 1 }
exit 0
