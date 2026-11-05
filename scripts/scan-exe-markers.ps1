#requires -Version 7
<#
.SYNOPSIS
  扫描 exe 中的构建标记（windows-plan M-7 / S-R4 / S-2；Ellie PR55、PR58）。

.DESCRIPTION
  同时按 UTF-8/ASCII 与 UTF-16LE 两种编码查找（Rust 字面量是 UTF-8；UTF-16LE 防止标记以宽字符形式出现）。
  正向对照（防止扫描器空转，任何构建都必须找到）：
    - UTF-8：CLASH_ASSIST_SCAN_POSITIVE_CONTROL（update_check/mod.rs 的专用常量，#[used] + 启动时 black_box 取址，
      不依赖业务字符串是否被编译器保留）；
    - UTF-16LE：VS_VERSION_INFO（Windows 版本资源的固定键名，tauri-build 总会嵌入版本资源）。
  标记与构建类型（-Build）：
    - Release（tag / 演练的 release exe）：CLASH_ASSIST_FAKE_LATEST 与 CLASH_ASSIST_UPDATER_E2E_BUILD 出现任何一个都失败
      （M5 起 FAKE_LATEST 只在 debug 与 updater-e2e 构建中编译，M-7）；
    - Debug（ci.yml 的 debug exe）：允许 FAKE_LATEST；出现 E2E 标记失败（CI 不得启用 updater-e2e feature）；
    - E2E（updater-e2e.yml 的 `--features updater-e2e` 构建）：必须含 E2E 标记（证明测的是 e2e 构建），允许 FAKE_LATEST。

  用法：pwsh scripts/scan-exe-markers.ps1 -Exe <path> -Build Release|Debug|E2E
        pwsh scripts/scan-exe-markers.ps1 -SelfTest
  退出码：0 = 通过；1 = 标记不符或正向对照失败；2 = 用法错误。
#>
[CmdletBinding(DefaultParameterSetName = 'Scan')]
param(
  [Parameter(Mandatory, ParameterSetName = 'Scan')][string]$Exe,
  [Parameter(Mandatory, ParameterSetName = 'Scan')][ValidateSet('Release', 'Debug', 'E2E')][string]$Build,
  [Parameter(Mandatory, ParameterSetName = 'SelfTest')][switch]$SelfTest
)
$ErrorActionPreference = 'Stop'

$Utf8Control = 'CLASH_ASSIST_SCAN_POSITIVE_CONTROL'
$Utf16Control = 'VS_VERSION_INFO'
$E2eMarker = 'CLASH_ASSIST_UPDATER_E2E_BUILD'
$FakeMarker = 'CLASH_ASSIST_FAKE_LATEST'

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
function Invoke-Scan([byte[]]$bytes, [string]$build) {
  $hay = Get-Haystack $bytes
  $lines = [Collections.Generic.List[string]]::new()
  $code = 0
  $lines.Add("构建类型：$build")
  if (-not (Test-Utf8 $hay $Utf8Control)) { $lines.Add("::error::正向对照失败：找不到 UTF-8 字符串 $Utf8Control，扫描器无效"); $code = 1 }
  else { $lines.Add("正向对照通过（UTF-8）：$Utf8Control") }
  if (-not (Test-Utf16 $hay $Utf16Control)) { $lines.Add("::error::正向对照失败：找不到 UTF-16LE 字符串 $Utf16Control，扫描器无效"); $code = 1 }
  else { $lines.Add("正向对照通过（UTF-16LE）：$Utf16Control") }

  $hits = Find-Marker $hay $E2eMarker
  if ($build -eq 'E2E') {
    if ($hits.Count) { $lines.Add("含 $E2eMarker（$($hits -join '、')），确认是 updater-e2e 构建") }
    else { $lines.Add("::error::E2E 构建中找不到 $E2eMarker：没有启用 updater-e2e feature，e2e 结果无效"); $code = 1 }
  } elseif ($hits.Count) {
    $lines.Add("::error::$build 构建中含 $E2eMarker（$($hits -join '、')），只有 updater-e2e.yml 可以启用 updater-e2e feature"); $code = 1
  } else { $lines.Add("不含 $E2eMarker（UTF-8 / UTF-16LE）") }

  $hits = Find-Marker $hay $FakeMarker
  if ($build -eq 'Release') {
    if ($hits.Count) { $lines.Add("::error::release 构建中含 $FakeMarker（$($hits -join '、')），M-7 cfg 门控失效"); $code = 1 }
    else { $lines.Add("不含 $FakeMarker（UTF-8 / UTF-16LE）") }
  } else {
    $lines.Add("$build 构建允许 $FakeMarker（$(if ($hits.Count) { "存在：$($hits -join '、')" } else { '未出现' })）")
  }
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
  $e8 = $u8.GetBytes($E2eMarker); $e16 = $u16.GetBytes($E2eMarker)
  $f8 = $u8.GetBytes($FakeMarker); $f16 = $u16.GetBytes($FakeMarker)
  $cases = @(
    @{ Name = 'Release 干净 + 两种对照'; Parts = @($c8, $c16); Build = 'Release'; Want = 0 }
    @{ Name = 'Debug 干净'; Parts = @($c8, $c16); Build = 'Debug'; Want = 0 }
    @{ Name = '缺 UTF-8 对照'; Parts = @($c16); Build = 'Release'; Want = 1 }
    @{ Name = '缺 UTF-16LE 对照'; Parts = @($c8); Build = 'Release'; Want = 1 }
    @{ Name = 'Debug 也要正向对照'; Parts = @($c16, $f8); Build = 'Debug'; Want = 1 }
    @{ Name = 'E2E 也要正向对照'; Parts = @($c8, $e8); Build = 'E2E'; Want = 1 }
    @{ Name = 'UTF-16 对照只有 UTF-8 形式不算'; Parts = @($c8, $u8.GetBytes($Utf16Control)); Build = 'Release'; Want = 1 }
    @{ Name = '旧 UTF-8 对照（LATEST_RELEASE_API）不再算数'; Parts = @($u8.GetBytes('https://api.github.com/repos/JTee77/clash-assist/releases/latest'), $c16); Build = 'Release'; Want = 1 }
    @{ Name = 'Release 含 E2E 标记 UTF-8'; Parts = @($c8, $c16, $e8); Build = 'Release'; Want = 1 }
    @{ Name = 'Release 含 E2E 标记 UTF-16LE'; Parts = @($c8, $c16, $e16); Build = 'Release'; Want = 1 }
    @{ Name = 'Release 含 FAKE_LATEST UTF-8'; Parts = @($c8, $c16, $f8); Build = 'Release'; Want = 1 }
    @{ Name = 'Release 含 FAKE_LATEST UTF-16LE'; Parts = @($c8, $c16, $f16); Build = 'Release'; Want = 1 }
    @{ Name = 'Release 奇数偏移的 UTF-16LE 也能找到'; Parts = @($c8, $c16, ([byte[]](, 0x41) + $f16)); Build = 'Release'; Want = 1 }
    @{ Name = 'Debug 允许 FAKE_LATEST'; Parts = @($c8, $c16, $f8); Build = 'Debug'; Want = 0 }
    @{ Name = 'Debug 含 E2E 标记失败'; Parts = @($c8, $c16, $f8, $e8); Build = 'Debug'; Want = 1 }
    @{ Name = 'Debug 含 E2E 标记 UTF-16LE 失败'; Parts = @($c8, $c16, $e16); Build = 'Debug'; Want = 1 }
    @{ Name = 'E2E 含 E2E 标记 + FAKE_LATEST 通过'; Parts = @($c8, $c16, $e8, $f8); Build = 'E2E'; Want = 0 }
    @{ Name = 'E2E 缺 E2E 标记失败'; Parts = @($c8, $c16, $f8); Build = 'E2E'; Want = 1 }
  )
  $fail = 0
  foreach ($c in $cases) {
    $r = Invoke-Scan (Build $c.Parts) $c.Build
    if ($r.Code -eq $c.Want) { Write-Host "ok   $($c.Name)" }
    else { Write-Host "FAIL $($c.Name)：code=$($r.Code)（期望 $($c.Want)）"; $r.Lines | ForEach-Object { Write-Host "     $_" }; $fail = 1 }
  }
  Write-Host ("scan-exe-markers 自测：" + ($(if ($fail) { '失败' } else { '全部通过' })))
  exit $fail
}

if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) { Write-Host "::error::找不到 $Exe"; exit 2 }
$r = Invoke-Scan ([IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Exe))) $Build
$r.Lines | ForEach-Object { Write-Host $_ }
exit $r.Code
