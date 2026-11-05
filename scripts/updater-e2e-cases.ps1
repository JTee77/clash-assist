#requires -Version 7.2
<#
.SYNOPSIS
  M5 更新器端到端用例（windows-plan r4 §6 M5「updater-e2e.yml（Nate）」；M-5、M-7、S-6、S-10、S-R4、S-R6、C-3）。

.DESCRIPTION
  在一次性 Windows runner 上，用 updater-e2e feature 构建的 0.3.0 / 0.3.1 安装包跑用例。
  每个用例的结果是 pass / fail / pending 之一，写入 -ResultsDir\results.json 与 job summary：

  已实现（不依赖 M5 更新器核心）：
    build-versions      两个安装包静默安装后，DisplayVersion 与主程序 PE ProductVersion 符合各自版本
    sr4-marker          安装版与便携版 exe 都含 CLASH_ASSIST_UPDATER_E2E_BUILD（S-R4 正向断言）
    server-windows      在 Windows 上起 updater-e2e-server.ps1（HttpListener，127.0.0.1），由真实 release JSON
                        回放出的四种 JSON（Happy / WrongDigest / WrongSize / ImmutableFalse）与安装包都能按预期取回（S-R6）
    installer-upgrade   直接运行 0.3.1 安装包 /S /UPDATE 覆盖 0.3.0，版本变为 0.3.1（一键更新最后一步的安装器侧）
    allow-downgrades    已装 0.3.1 时直接运行 0.3.0 安装包（/S，以及客户端会用的 /S /UPDATE），
                        必须返回非 0 且版本不变（allowDowngrades: false，M-5）

  pending（依赖 M5 更新器核心，见各用例的 Needs；不会被记为通过）：
    happy               0.3.0 一键更新到 0.3.1：下载、校验、/S /UPDATE /R、重启，ProductVersion 0.3.1、新进程、pending 标记已清除
    wrong-digest        digest 错误 → 失败、不退出、版本不变
    wrong-size          size 错误 → 同上
    immutable-false     immutable: false → 拒绝一键更新（C-3）
    pe-mismatch         JSON 声称 v0.3.2、实际提供 0.3.1 安装包 → S-6 拒绝
    downgrade           已装 0.3.1，JSON 声称 v0.3.2、实际提供 0.3.0 安装包 → 拒绝
    portable-one-click  便携版 one_click == false（S-10）

  测试前若机器上已装有 Clash Assist 或已有 %LOCALAPPDATA%\ClashAssist，脚本拒绝运行。

.EXAMPLE
  pwsh scripts/updater-e2e-cases.ps1 -Dist030 e2e/0.3.0 -Dist031 e2e/0.3.1 `
       -Fixture src-tauri/tests/fixtures/release-clash-assist-v0.2.9.json -ResultsDir $env:RUNNER_TEMP/e2e-results
  退出码：0 = 没有 fail（可以有 pending）；1 = 有 fail；2 = 用法 / 环境错误。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)][string]$Dist030,
  [Parameter(Mandatory)][string]$Dist031,
  [Parameter(Mandatory)][string]$Fixture,
  [Parameter(Mandatory)][string]$ResultsDir,
  [int]$UninstallTimeoutSeconds = 120
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { Write-Host '::error::updater-e2e-cases.ps1 只能在 Windows 上运行'; exit 2 }

$ProductName = 'Clash Assist'
$UninstKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProductName"
$ExpectedInstallDir = Join-Path $env:LOCALAPPDATA $ProductName
$AppDataDir = Join-Path $env:LOCALAPPDATA 'ClashAssist'
$Here = $PSScriptRoot
$Server = Join-Path $Here 'updater-e2e-server.ps1'
$Scanner = Join-Path $Here 'scan-exe-markers.ps1'

$results = [Collections.Generic.List[object]]::new()
function Add-Result([string]$name, [string]$status, [string]$detail, [string]$needs = '') {
  $results.Add([pscustomobject]@{ case = $name; status = $status; detail = $detail; needs = $needs })
  $tagText = switch ($status) { 'pass' { 'ok     ' } 'fail' { 'FAIL   ' } default { 'PENDING' } }
  Write-Host "$tagText $name：$detail$(if ($needs) { "（依赖：$needs）" })"
  if ($status -eq 'fail') { Write-Host "::error::$name：$detail" }
  if ($status -eq 'pending') { Write-Host "::warning::$name 未实现，依赖 M5 更新器核心：$needs" }
}
# 运行一个用例：脚本块返回失败原因的列表（空 = 通过）；抛异常算失败
function Invoke-Case([string]$name, [scriptblock]$body) {
  Write-Host "`n=== $name"
  try {
    $errs = @(& $body)
    if ($errs.Count -eq 0) { Add-Result $name 'pass' '通过' } else { Add-Result $name 'fail' ($errs -join '；') }
  } catch {
    Add-Result $name 'fail' "异常：$($_.Exception.Message)"
  }
}

function Get-Dist([string]$dir, [string]$ver) {
  $setup = Join-Path $dir "Clash.Assist_${ver}_x64-setup.exe"
  $zip = Join-Path $dir "Clash.Assist_${ver}_x64-portable.zip"
  foreach ($f in $setup, $zip) { if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { throw "找不到 $f" } }
  @{ Setup = (Resolve-Path -LiteralPath $setup).Path; Zip = (Resolve-Path -LiteralPath $zip).Path; Version = $ver }
}

function Read-UninstallKey { if (Test-Path -LiteralPath $UninstKey) { Get-ItemProperty -LiteralPath $UninstKey } else { $null } }

# 当前安装：@{ Version = DisplayVersion; Exe = 主程序路径; Pe = ProductVersion } 或 $null
function Get-Installed {
  $k = Read-UninstallKey
  if (-not $k) { return $null }
  $dir = ([string]$k.InstallLocation).Trim('"')
  $exe = Join-Path $dir ([string]$k.MainBinaryName)
  $pe = if (Test-Path -LiteralPath $exe -PathType Leaf) { (Get-Item -LiteralPath $exe).VersionInfo.ProductVersion } else { $null }
  @{ Version = [string]$k.DisplayVersion; Exe = $exe; Dir = $dir; Pe = $pe }
}

function Invoke-Setup([string]$setup, [string[]]$args_) {
  $p = Start-Process -FilePath $setup -ArgumentList $args_ -Wait -PassThru
  Write-Host "  $(Split-Path -Leaf $setup) $($args_ -join ' ') → 退出码 $($p.ExitCode)"
  $p.ExitCode
}

function Uninstall-All {
  $i = Get-Installed
  if (-not $i) { return $true }
  Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($i.Dir, [StringComparison]::OrdinalIgnoreCase) } |
    Stop-Process -Force -ErrorAction SilentlyContinue
  $u = Join-Path $i.Dir 'uninstall.exe'
  if (Test-Path -LiteralPath $u) { Start-Process -FilePath $u -ArgumentList '/S' -Wait | Out-Null }
  # NSIS 卸载程序会把自己复制到 %TEMP% 再运行，原进程立即返回，所以按结果轮询
  $deadline = (Get-Date).AddSeconds($UninstallTimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    if (-not (Read-UninstallKey) -and -not (Test-Path -LiteralPath $u)) { return $true }
    Start-Sleep -Seconds 2
  }
  $false
}

# PE ProductVersion 与目标版本的比较方式属于 S-6 的实现（Pit）；这里只接受 X.Y.Z 或 X.Y.Z.0 两种写法并打印实际值
function Test-PeVersion([string]$pe, [string]$ver) { $pe -in @($ver, "$ver.0") }

function Get-FreePort {
  $l = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
  $l.Start(); $p = ([Net.IPEndPoint]$l.LocalEndpoint).Port; $l.Stop(); $p
}

function Start-E2EServer([string]$root, [string]$tag, [int]$port) {
  $ready = Join-Path $ResultsDir "ready-$tag-$([Guid]::NewGuid().ToString('N')).txt"
  $log = Join-Path $ResultsDir "requests-$tag.jsonl"
  $pwsh = (Get-Process -Id $PID).Path
  $p = Start-Process -FilePath $pwsh -PassThru -NoNewWindow -ArgumentList @('-NoProfile', '-File', $Server, '-Serve', '-Root', $root, '-Port', "$port", '-ReadyFile', $ready, '-RequestLog', $log, '-MaxSeconds', '600')
  $t0 = Get-Date
  while (-not (Test-Path -LiteralPath $ready) -and ((Get-Date) - $t0).TotalSeconds -lt 30 -and -not $p.HasExited) { Start-Sleep -Milliseconds 200 }
  if (-not (Test-Path -LiteralPath $ready)) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; throw 'updater-e2e-server 未就绪' }
  @{ Proc = $p; Port = [int](Get-Content -Raw -LiteralPath $ready); Log = $log }
}

# ---------- 准备 ----------
New-Item -ItemType Directory -Force -Path $ResultsDir | Out-Null
$ResultsDir = (Resolve-Path -LiteralPath $ResultsDir).Path
if (Read-UninstallKey) { Write-Host "::error::本机已安装 $ProductName（$UninstKey 存在），拒绝运行"; exit 2 }
if (Test-Path -LiteralPath $AppDataDir) { Write-Host "::error::$AppDataDir 已存在，拒绝运行（避免改动已有的应用数据）"; exit 2 }
$d0 = Get-Dist $Dist030 '0.3.0'
$d1 = Get-Dist $Dist031 '0.3.1'
$Fixture = (Resolve-Path -LiteralPath $Fixture).Path
$portableDir = Join-Path ([IO.Path]::GetTempPath()) ("ca-e2e-portable-" + [Guid]::NewGuid().ToString('N'))

try {
  Invoke-Case 'build-versions' {
    $e = @()
    foreach ($d in $d0, $d1) {
      $code = Invoke-Setup $d.Setup @('/S')
      if ($code -ne 0) { $e += "$($d.Version) /S 退出码 $code"; continue }
      $i = Get-Installed
      if (-not $i) { $e += "$($d.Version) 安装后没有 Uninstall 键"; continue }
      Write-Host "  DisplayVersion=$($i.Version) ProductVersion=$($i.Pe) exe=$($i.Exe)"
      if ($i.Version -ne $d.Version) { $e += "$($d.Version)：DisplayVersion 为 $($i.Version)" }
      if (-not (Test-PeVersion $i.Pe $d.Version)) { $e += "$($d.Version)：ProductVersion 为 $($i.Pe)" }
      if (-not (Uninstall-All)) { $e += "$($d.Version) 卸载未完成" }
    }
    $e
  }

  Invoke-Case 'sr4-marker' {
    $e = @()
    $code = Invoke-Setup $d0.Setup @('/S')
    if ($code -ne 0) { return @("0.3.0 /S 退出码 $code") }
    $i = Get-Installed
    & $Scanner -Exe $i.Exe -RequireE2EMarker
    if ($LASTEXITCODE -ne 0) { $e += "安装版 exe 正向断言失败（$($i.Exe)）" }
    if (-not (Uninstall-All)) { $e += '卸载未完成' }
    Expand-Archive -LiteralPath $d0.Zip -DestinationPath $portableDir
    $pexe = @(Get-ChildItem -LiteralPath $portableDir -Filter '*.exe' -File)
    if ($pexe.Count -ne 1) { $e += "便携 zip 根目录应恰有一个 exe（实际 $($pexe.Count)）" }
    else {
      & $Scanner -Exe $pexe[0].FullName -RequireE2EMarker
      if ($LASTEXITCODE -ne 0) { $e += "便携版 exe 正向断言失败（$($pexe[0].FullName)）" }
    }
    $e
  }

  Invoke-Case 'server-windows' {
    $e = @()
    $sha = (Get-FileHash -Algorithm SHA256 -LiteralPath $d1.Setup).Hash.ToLowerInvariant()
    $size = (Get-Item -LiteralPath $d1.Setup).Length
    # 端口要写进 JSON 的 browser_download_url：先选空闲端口，按它回放，再用同一端口起服务器
    foreach ($case in 'Happy', 'WrongDigest', 'WrongSize', 'ImmutableFalse') {
      $port = Get-FreePort
      $root = Join-Path $ResultsDir "root-$case"
      & $Server -Replay -Fixture $Fixture -Tag v0.3.1 -AssetFile $d1.Setup -BaseUrl "http://127.0.0.1:$port" -Case $case -OutDir $root
      if ($LASTEXITCODE -ne 0) { $e += "$case：Replay 失败"; continue }
      $srv = Start-E2EServer $root $case $port
      try {
        if ($srv.Port -ne $port) { $e += "$case：端口变化（$port → $($srv.Port)）"; continue }
        $j = Invoke-RestMethod -Uri "http://127.0.0.1:$port/release.json"
        $a = @($j.assets)
        if ($a.Count -ne 1) { $e += "$case：资产数 $($a.Count)"; continue }
        $tmp = Join-Path $ResultsDir "dl-$case.exe"
        Invoke-WebRequest -Uri $a[0].browser_download_url -OutFile $tmp
        $gotSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $tmp).Hash.ToLowerInvariant()
        Remove-Item -LiteralPath $tmp -Force
        if ($gotSha -ne $sha) { $e += "$case：下载内容与 0.3.1 安装包不一致" }
        $digestOk = $a[0].digest -eq "sha256:$sha"; $sizeOk = [long]$a[0].size -eq $size
        if ($digestOk -ne ($case -ne 'WrongDigest')) { $e += "$case：digest=$($a[0].digest)" }
        if ($sizeOk -ne ($case -ne 'WrongSize')) { $e += "$case：size=$($a[0].size)" }
        if ([bool]$j.immutable -ne ($case -ne 'ImmutableFalse') -or $j.immutable -isnot [bool]) { $e += "$case：immutable=$($j.immutable)" }
        if ($j.tag_name -ne 'v0.3.1' -or $a[0].name -ne 'Clash.Assist_0.3.1_x64-setup.exe') { $e += "$case：tag / 资产名不符" }
        Write-Host "  $case：端口 $port，digest $(if ($digestOk) { '一致' } else { '不一致' })，size $(if ($sizeOk) { '一致' } else { '不一致' })，immutable=$($j.immutable)"
      } finally {
        Stop-Process -Id $srv.Proc.Id -Force -ErrorAction SilentlyContinue
      }
    }
    $e
  }

  Invoke-Case 'installer-upgrade' {
    $e = @()
    $code = Invoke-Setup $d0.Setup @('/S'); if ($code -ne 0) { return @("0.3.0 /S 退出码 $code") }
    $code = Invoke-Setup $d1.Setup @('/S', '/UPDATE')
    if ($code -ne 0) { $e += "0.3.1 /S /UPDATE 退出码 $code" }
    $i = Get-Installed
    Write-Host "  升级后 DisplayVersion=$($i.Version) ProductVersion=$($i.Pe)"
    if ($i.Version -ne '0.3.1' -or -not (Test-PeVersion $i.Pe '0.3.1')) { $e += "升级后版本为 $($i.Version) / $($i.Pe)" }
    # 保持 0.3.1 已安装，供 allow-downgrades 使用
    $e
  }

  Invoke-Case 'allow-downgrades' {
    $e = @()
    $i = Get-Installed
    if (-not $i -or $i.Version -ne '0.3.1') {
      $code = Invoke-Setup $d1.Setup @('/S'); if ($code -ne 0) { return @("0.3.1 /S 退出码 $code") }
    }
    foreach ($a in @(, @('/S')) + @(, @('/S', '/UPDATE'))) {
      $code = Invoke-Setup $d0.Setup $a
      $i = Get-Installed
      Write-Host "  之后 DisplayVersion=$($i.Version) ProductVersion=$($i.Pe)"
      if ($code -eq 0) { $e += "0.3.0 $($a -join ' ') 覆盖 0.3.1 时退出码为 0（应为非 0，allowDowngrades: false）" }
      if (-not $i -or $i.Version -ne '0.3.1' -or -not (Test-PeVersion $i.Pe '0.3.1')) { $e += "0.3.0 $($a -join ' ') 之后版本为 $($i.Version) / $($i.Pe)（应仍为 0.3.1）" }
      if (-not $i) { break }
      if ($i.Version -ne '0.3.1') {
        # 已被降级：恢复 0.3.1，继续下一个参数组合
        [void](Uninstall-All); [void](Invoke-Setup $d1.Setup @('/S'))
      }
    }
    if (-not (Uninstall-All)) { $e += '卸载未完成' }
    $e
  }

  # ---------- 依赖 M5 更新器核心的用例：明确记为 pending，不伪造结果 ----------
  $core = 'CLASH_ASSIST_E2E_API 端点覆盖与本地下载白名单（§2.4 M-7 ①②）；无界面触发 check → download_and_verify → launch_installer 的方式；可断言的结果信号（退出码 / 日志行）'
  Add-Result 'happy' 'pending' '0.3.0 → 0.3.1 一键更新（下载、校验、/S /UPDATE /R、重启、ProductVersion、新进程、pending 标记清除）' "$core；update-pending.json 的路径与清除时机（M-5）"
  Add-Result 'wrong-digest' 'pending' 'digest 错误应失败、不退出、版本不变（JSON 已由 server-windows 验证可生成）' $core
  Add-Result 'wrong-size' 'pending' 'size 错误应失败、不退出、版本不变' $core
  Add-Result 'immutable-false' 'pending' 'immutable: false 应拒绝一键更新、只给发布页（C-3）' "$core；拒绝时对外可见的状态（one_click / 错误码）"
  Add-Result 'pe-mismatch' 'pending' 'JSON 声称 v0.3.2、提供 0.3.1 安装包，应被 S-6 拒绝' "$core；PE ProductVersion 与 tag 的比较规则（X.Y.Z / X.Y.Z.0）"
  Add-Result 'downgrade' 'pending' '已装 0.3.1，JSON 声称 v0.3.2、提供 0.3.0 安装包，应被拒绝' $core
  Add-Result 'portable-one-click' 'pending' '便携版 UpdateInfo.one_click == false（S-10）' '从进程外读取 install_kind / one_click 的方式（日志行或 e2e 专用输出）'
} finally {
  Write-Host "`n=== 清理"
  [void](Uninstall-All)
  Remove-Item -LiteralPath $portableDir -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $AppDataDir -Recurse -Force -ErrorAction SilentlyContinue
  $results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $ResultsDir 'results.json') -Encoding utf8
  if ($env:GITHUB_STEP_SUMMARY) {
    $md = @('### updater-e2e 用例', '', '| 用例 | 结果 | 说明 | 依赖 |', '|---|---|---|---|')
    foreach ($r in $results) { $md += "| $($r.case) | $($r.status) | $($r.detail -replace '\|', '/') | $($r.needs -replace '\|', '/') |" }
    $md += ''; $md += 'pending 表示依赖 M5 更新器核心、尚未实现，**不是通过**。'
    ($md -join "`n") | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Append -Encoding utf8
  }
}

$fail = @($results | Where-Object status -eq 'fail').Count
$pending = @($results | Where-Object status -eq 'pending').Count
$pass = @($results | Where-Object status -eq 'pass').Count
Write-Host "`nupdater-e2e：通过 $pass，失败 $fail，pending $pending（pending 依赖 M5 更新器核心，不计为通过）"
exit $(if ($fail) { 1 } else { 0 })
