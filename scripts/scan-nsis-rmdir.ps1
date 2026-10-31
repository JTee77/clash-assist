#requires -Version 7
<#
.SYNOPSIS
  扫描 tauri 生成的 installer.nsi：不得有作用在 APPDATA / LOCALAPPDATA 上的 `RmDir /r`（Ellie PR55 r2）。

.DESCRIPTION
  NSIS 3.10 的 `RmDir /r` 会跟随 junction（scripts/test-nsis-hooks.ps1 实测）。我们自己的 hooks.nsh 已改用
  不跟随链接的 un.CA_RmTreeNoFollow；Tauri 模板自带的「删除应用数据」仍是
      SetShellVarContext current
      RmDir /r "$APPDATA\${BUNDLEID}"
      RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
  在修掉它之前（win/m4b-nsis-nofollow），tag 构建（-Strict）必须在这里失败，演练只记 warning。

  判定：去掉注释、拼接以 `\` 结尾的续行后，凡是 RMDir 指令（不分大小写）的开关里有 `/r`，且路径参数
  引用了下列任一写法，即为命中：
    $APPDATA  $LOCALAPPDATA  ${APPDATA}  ${LOCALAPPDATA}（NSIS 常量，SetShellVarContext 只改变其解析结果）
    $%APPDATA%  $%LOCALAPPDATA%（环境变量写法）
    任何含 `AppData` 字样的路径（如 $PROFILE\AppData\Local\…）
  其余 `RmDir /r` 只列出不判失败（如 $INSTDIR 下的目录）。

  用法：
    pwsh scripts/scan-nsis-rmdir.ps1 -Nsi <installer.nsi> [-Strict]
    pwsh scripts/scan-nsis-rmdir.ps1 -SelfTest
  退出码：0 = 通过（或非 strict 下只有 warning）；1 = strict 下命中；2 = 文件不存在或读不了。
#>
[CmdletBinding(DefaultParameterSetName = 'Scan')]
param(
  [Parameter(Mandatory, ParameterSetName = 'Scan')] [string] $Nsi,
  [Parameter(ParameterSetName = 'Scan')] [switch] $Strict,
  [Parameter(Mandatory, ParameterSetName = 'SelfTest')] [switch] $SelfTest
)
$ErrorActionPreference = 'Stop'

$AppDataRef = '(?i)\$\{?(LOCAL)?APPDATA\}?(?![A-Za-z0-9_])|\$%(LOCAL)?APPDATA%|AppData'

# 返回 @{ Hits = 命中的行; Others = 其他 RmDir /r 行 }
function Find-RecursiveRmDir([string] $text) {
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

  $hits = @(); $others = @()
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
    # 指令可能在 LogicLib 之后或带标签，这里只认行首的 RMDir（含 un. 区段里的同一指令）
    if ($t -notmatch '^(?i)RMDir\b(?<rest>.*)$') { continue }
    $rest = $Matches.rest.Trim()
    $switches = [regex]::Matches($rest, '(?<=^|\s)/[A-Za-z]+') | ForEach-Object { $_.Value.ToLowerInvariant() }
    if ($switches -notcontains '/r') { continue }
    $entry = "第 $($ln.No) 行：$t"
    if ($rest -match $AppDataRef) { $hits += $entry } else { $others += $entry }
  }
  @{ Hits = $hits; Others = $others }
}

if ($SelfTest) {
  $cases = @(
    @{ Name = 'Tauri 模板原样'; Text = "  SetShellVarContext current`n  RmDir /r `"`$APPDATA\`${BUNDLEID}`"`n  RmDir /r `"`$LOCALAPPDATA\`${BUNDLEID}`""; Hits = 2 }
    @{ Name = '小写与开关顺序'; Text = "rmdir /REBOOTOK /r `$LocalAppData\x"; Hits = 1 }
    @{ Name = '花括号写法'; Text = "RMDir /r `"`${LOCALAPPDATA}\x`""; Hits = 1 }
    @{ Name = '环境变量写法'; Text = "RMDir /r `"`$%APPDATA%\x`""; Hits = 1 }
    @{ Name = 'PROFILE\AppData'; Text = "RMDir /r `"`$PROFILE\AppData\Local\x`""; Hits = 1 }
    @{ Name = '续行'; Text = "RMDir /r \`n  `"`$APPDATA\x`""; Hits = 1 }
    @{ Name = '不递归的 RMDir'; Text = "RMDir `"`$APPDATA\x`"`nRMDir /REBOOTOK `"`$LOCALAPPDATA\x`""; Hits = 0 }
    @{ Name = '注释掉的'; Text = "; RmDir /r `"`$APPDATA\x`"`n# RmDir /r `$LOCALAPPDATA`n/* RmDir /r `$APPDATA`n*/"; Hits = 0 }
    @{ Name = 'INSTDIR 只列出'; Text = "RMDir /r `"`$INSTDIR\resources`""; Hits = 0; Others = 1 }
    @{ Name = '含 AppData 字样一律算（宁严勿松）'; Text = "RMDir /r `"`$APPDATAX\x`""; Hits = 1 }
    @{ Name = '我们的 hooks（不用 /r）'; Text = "Push `"`$LOCALAPPDATA\ClashAssist`"`nCall un.CA_RmTreeNoFollow`nRMDir `"`$R0`""; Hits = 0 }
  )
  $fail = 0
  foreach ($c in $cases) {
    $r = Find-RecursiveRmDir $c.Text
    $ok = ($r.Hits.Count -eq $c.Hits) -and (-not $c.ContainsKey('Others') -or $r.Others.Count -eq $c.Others)
    if ($ok) { Write-Host "ok   $($c.Name)" } else { Write-Host "FAIL $($c.Name)：命中 $($r.Hits.Count)（期望 $($c.Hits)），其他 $($r.Others.Count)"; $fail = 1 }
  }
  Write-Host ('scan-nsis-rmdir 自测：' + ($(if ($fail) { '失败' } else { '全部通过' })))
  exit $fail
}

if (-not (Test-Path -LiteralPath $Nsi -PathType Leaf)) { Write-Host "::error::找不到 $Nsi"; exit 2 }
$r = Find-RecursiveRmDir (Get-Content -Raw -LiteralPath $Nsi)
foreach ($o in $r.Others) { Write-Host "  其他 RmDir /r（不判失败）$o" }
if (-not $r.Hits.Count) { Write-Host "scan-nsis-rmdir：$Nsi 中没有作用在 APPDATA / LOCALAPPDATA 上的 RmDir /r"; exit 0 }
$level = if ($Strict) { 'error' } else { 'warning' }
foreach ($h in $r.Hits) {
  Write-Host "::${level}::installer.nsi 用 RmDir /r 删除应用数据目录（会跟随 junction，修复见 win/m4b-nsis-nofollow）：$h"
}
if ($Strict) { exit 1 }
exit 0
