#requires -Version 7
<#
.SYNOPSIS
  扫描 release exe 中不应出现的测试标记（windows-plan M-7 / S-R4；Ellie PR55）。

.DESCRIPTION
  同时按 UTF-8/ASCII 与 UTF-16LE 两种编码查找（Rust 字面量是 UTF-8；UTF-16LE 防止标记以宽字符形式出现）。
  正向对照（防止扫描器空转）：
    - UTF-8：LATEST_RELEASE_API（update_check.rs 中必然存在的字符串）；
    - UTF-16LE：VS_VERSION_INFO（Windows 版本资源的固定键名，tauri-build 总会嵌入版本资源）。
  标记：
    - CLASH_ASSIST_UPDATER_E2E_BUILD：任何情况下出现都失败（不得启用 updater-e2e feature）；
    - CLASH_ASSIST_FAKE_LATEST：-Strict（tag 构建）时失败，否则只记 warning
      （M5 的 M-7 cfg 门控合入前它仍无条件编译）。
  -RequireE2EMarker（updater-e2e.yml，S-R4 正向断言）：反过来要求 exe 中**必须**含 UTF-8 形式的
  CLASH_ASSIST_UPDATER_E2E_BUILD（Rust 字节字面量），否则失败；此模式下 CLASH_ASSIST_FAKE_LATEST 只记录、不判定。
  与 release 构建的反向断言配对，防止标记被优化掉或拼错后反向检查空转。

  用法：pwsh scripts/scan-exe-markers.ps1 -Exe <path> [-Strict]
        pwsh scripts/scan-exe-markers.ps1 -Exe <path> -RequireE2EMarker
        pwsh scripts/scan-exe-markers.ps1 -SelfTest
  退出码：0 = 通过；1 = 发现标记或正向对照失败；2 = 用法错误。
#>
[CmdletBinding(DefaultParameterSetName = 'Scan')]
param(
  [Parameter(Mandatory, ParameterSetName = 'Scan')][string]$Exe,
  [Parameter(ParameterSetName = 'Scan')][switch]$Strict,
  [Parameter(ParameterSetName = 'Scan')][switch]$RequireE2EMarker,
  [Parameter(Mandatory, ParameterSetName = 'SelfTest')][switch]$SelfTest
)
$ErrorActionPreference = 'Stop'

$Utf8Control = 'https://api.github.com/repos/JTee77/clash-assist/releases/latest'
$Utf16Control = 'VS_VERSION_INFO'
$AlwaysBad = 'CLASH_ASSIST_UPDATER_E2E_BUILD'
$StrictBad = 'CLASH_ASSIST_FAKE_LATEST'

function Get-Haystack([byte[]]$bytes) { [Text.Encoding]::Latin1.GetString($bytes) }
function Test-Utf8([string]$hay, [string]$needle) {
  $hay.IndexOf([Text.Encoding]::Latin1.GetString([Text.Encoding]::UTF8.GetBytes($needle)), [StringComparison]::Ordinal) -ge 0
}
function Test-Utf16([string]$hay, [string]$needle) {
  $hay.IndexOf([Text.Encoding]::Latin1.GetString([Text.Encoding]::Unicode.GetBytes($needle)), [StringComparison]::Ordinal) -ge 0
}
function Find-Marker([string]$hay, [string]$needle) {
  $hits = @()
  if (Test-Utf8 $hay $needle) { $hits += 'UTF-8' }
  if (Test-Utf16 $hay $needle) { $hits += 'UTF-16LE' }
  , $hits
}

# 返回 @{ Code = 0/1; Lines = @(...) }
function Invoke-Scan([byte[]]$bytes, [bool]$strict, [bool]$requireE2E = $false) {
  $hay = Get-Haystack $bytes
  $lines = [Collections.Generic.List[string]]::new()
  $code = 0
  if (-not (Test-Utf8 $hay $Utf8Control)) { $lines.Add("::error::正向对照失败：找不到 UTF-8 字符串 $Utf8Control，扫描器无效"); $code = 1 }
  else { $lines.Add("正向对照通过（UTF-8）：$Utf8Control") }
  if (-not (Test-Utf16 $hay $Utf16Control)) { $lines.Add("::error::正向对照失败：找不到 UTF-16LE 字符串 $Utf16Control，扫描器无效"); $code = 1 }
  else { $lines.Add("正向对照通过（UTF-16LE）：$Utf16Control") }

  if ($requireE2E) {
    # S-R4 正向断言：e2e 构建必须含 UTF-8 标记
    if (Test-Utf8 $hay $AlwaysBad) { $lines.Add("正向断言通过（UTF-8）：e2e 构建含 $AlwaysBad") }
    else { $lines.Add("::error::正向断言失败：e2e 构建中找不到 UTF-8 字符串 $AlwaysBad（标记被优化掉、拼错，或没有启用 updater-e2e feature）"); $code = 1 }
    $hits = Find-Marker $hay $StrictBad
    $lines.Add("e2e 构建：$StrictBad " + $(if ($hits.Count) { "存在（$($hits -join '、')），不判定" } else { '不存在' }))
    return @{ Code = $code; Lines = $lines }
  }

  $hits = Find-Marker $hay $AlwaysBad
  if ($hits.Count) { $lines.Add("::error::release 构建中含 $AlwaysBad（$($hits -join '、')），不得启用 updater-e2e feature"); $code = 1 }
  else { $lines.Add("不含 $AlwaysBad（UTF-8 / UTF-16LE）") }

  $hits = Find-Marker $hay $StrictBad
  if ($hits.Count) {
    if ($strict) { $lines.Add("::error::release 构建中含 $StrictBad（$($hits -join '、')），M5 的 M-7 cfg 门控合入前不能发布"); $code = 1 }
    else { $lines.Add("::warning::exe 中含 $StrictBad（$($hits -join '、')）；M5 的 M-7 cfg 门控合入前，tag 构建会在这里失败") }
  } else { $lines.Add("不含 $StrictBad（UTF-8 / UTF-16LE）") }
  @{ Code = $code; Lines = $lines }
}

if ($SelfTest) {
  $u8 = [Text.Encoding]::UTF8; $u16 = [Text.Encoding]::Unicode
  $pad = [byte[]](0..255)
  function Build([object[]]$parts) {
    $ms = [IO.MemoryStream]::new()
    foreach ($p in $parts) { $b = [byte[]]$p; $ms.Write($b, 0, $b.Length); $ms.Write($pad, 0, $pad.Length) }
    , $ms.ToArray()
  }
  $c8 = $u8.GetBytes($Utf8Control); $c16 = $u16.GetBytes($Utf16Control)
  $cases = @(
    @{ Name = '干净 + 两种对照'; Parts = @($c8, $c16); Strict = $true; Want = 0 }
    @{ Name = '缺 UTF-8 对照'; Parts = @($c16); Strict = $false; Want = 1 }
    @{ Name = '缺 UTF-16LE 对照'; Parts = @($c8); Strict = $false; Want = 1 }
    @{ Name = 'UTF-16 对照只有 UTF-8 形式不算'; Parts = @($c8, $u8.GetBytes($Utf16Control)); Strict = $false; Want = 1 }
    @{ Name = 'E2E 标记 UTF-8'; Parts = @($c8, $c16, $u8.GetBytes($AlwaysBad)); Strict = $false; Want = 1 }
    @{ Name = 'E2E 标记 UTF-16LE'; Parts = @($c8, $c16, $u16.GetBytes($AlwaysBad)); Strict = $false; Want = 1 }
    @{ Name = 'FAKE_LATEST UTF-8，非 strict 只警告'; Parts = @($c8, $c16, $u8.GetBytes($StrictBad)); Strict = $false; Want = 0; Warn = $true }
    @{ Name = 'FAKE_LATEST UTF-8，strict 失败'; Parts = @($c8, $c16, $u8.GetBytes($StrictBad)); Strict = $true; Want = 1 }
    @{ Name = 'FAKE_LATEST UTF-16LE，strict 失败'; Parts = @($c8, $c16, $u16.GetBytes($StrictBad)); Strict = $true; Want = 1 }
    @{ Name = 'FAKE_LATEST UTF-16LE，非 strict 只警告'; Parts = @($c8, $c16, $u16.GetBytes($StrictBad)); Strict = $false; Want = 0; Warn = $true }
    @{ Name = '奇数偏移的 UTF-16LE 也能找到'; Parts = @($c8, $c16, ([byte[]](,0x41) + $u16.GetBytes($StrictBad))); Strict = $true; Want = 1 }
    # -RequireE2EMarker（S-R4 正向断言）
    @{ Name = 'e2e：含 UTF-8 标记通过'; Parts = @($c8, $c16, $u8.GetBytes($AlwaysBad)); Strict = $false; E2E = $true; Want = 0 }
    @{ Name = 'e2e：含标记与 FAKE_LATEST 也通过'; Parts = @($c8, $c16, $u8.GetBytes($AlwaysBad), $u8.GetBytes($StrictBad)); Strict = $false; E2E = $true; Want = 0 }
    @{ Name = 'e2e：缺标记失败'; Parts = @($c8, $c16); Strict = $false; E2E = $true; Want = 1 }
    @{ Name = 'e2e：标记只有 UTF-16LE 形式不算'; Parts = @($c8, $c16, $u16.GetBytes($AlwaysBad)); Strict = $false; E2E = $true; Want = 1 }
    @{ Name = 'e2e：拼错的标记不算'; Parts = @($c8, $c16, $u8.GetBytes('CLASH_ASSIST_UPDATER_E2E_BUIL')); Strict = $false; E2E = $true; Want = 1 }
    @{ Name = 'e2e：含标记但缺 UTF-8 对照仍失败'; Parts = @($c16, $u8.GetBytes($AlwaysBad)); Strict = $false; E2E = $true; Want = 1 }
  )
  $fail = 0
  foreach ($c in $cases) {
    $r = Invoke-Scan (Build $c.Parts) $c.Strict ([bool]$c['E2E'])
    $warned = [bool]($r.Lines | Where-Object { $_ -like '::warning::*' })
    $ok = ($r.Code -eq $c.Want) -and (-not $c.ContainsKey('Warn') -or $warned -eq $c.Warn)
    if ($ok) { Write-Host "ok   $($c.Name)" } else { Write-Host "FAIL $($c.Name)：code=$($r.Code)（期望 $($c.Want)）"; $r.Lines | ForEach-Object { Write-Host "     $_" }; $fail = 1 }
  }
  Write-Host ("scan-exe-markers 自测：" + ($(if ($fail) { '失败' } else { '全部通过' })))
  exit $fail
}

if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) { Write-Host "::error::找不到 $Exe"; exit 2 }
if ($Strict -and $RequireE2EMarker) { Write-Host '::error::-Strict 与 -RequireE2EMarker 不能同时使用'; exit 2 }
$r = Invoke-Scan ([IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Exe))) ([bool]$Strict) ([bool]$RequireE2EMarker)
$r.Lines | ForEach-Object { Write-Host $_ }
exit $r.Code
