#requires -Version 7.2
<#
.SYNOPSIS
  M5 更新器端到端用例（windows-plan r4 §6 M5；M-5、M-7、S-6、S-10、S-R4、S-R6、C-3）。

.DESCRIPTION
  在一次性 Windows runner 上，用 updater-e2e feature 构建的 0.3.0 和 0.3.1 跑用例。
  客户端接口按 docs/windows-updater.md「e2e 接口约定」：
    - CLASH_ASSIST_E2E_API=http://127.0.0.1:<port>/release.json
    - CLASH_ASSIST_E2E_AUTO_UPDATE=1：启动后无界面执行 check → download_and_verify → launch_installer
    - 信号：%LOCALAPPDATA%\ClashAssist\logs\app.log 中的 [update] 行；标记 update-pending.json；工作目录 updates\<32 hex>\
  「安装程序有没有被启动」靠进程创建审计（Security 4688，含命令行）判断；workflow 只在这台 runner 上开启审计，
  audit-control 用例做正向对照（审计不可用时，依赖它的断言一律失败，不会放行）。

  每个用例的结果为 pass 或 fail，写入 -ResultsDir\results.json 与 job summary。没有 pending。
  已有 Clash Assist 安装或 %LOCALAPPDATA%\ClashAssist 时拒绝运行。

  退出码：0 = 全部通过；1 = 有失败；2 = 用法或环境错误。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)][string]$Dist030,
  [Parameter(Mandatory)][string]$Dist031,
  [Parameter(Mandatory)][string]$Fixture,
  [Parameter(Mandatory)][string]$ResultsDir,
  [int]$UninstallTimeoutSeconds = 120,
  [int]$AppSignalTimeoutSeconds = 90
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { Write-Host '::error::updater-e2e-cases.ps1 只能在 Windows 上运行'; exit 2 }

$ProductName = 'Clash Assist'
$UninstKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProductName"
$AppDataDir = Join-Path $env:LOCALAPPDATA 'ClashAssist'
$AppLog = Join-Path $AppDataDir 'logs\app.log'
$Marker = Join-Path $AppDataDir 'update-pending.json'
$UpdatesDir = Join-Path $AppDataDir 'updates'
$Here = $PSScriptRoot
$Server = Join-Path $Here 'updater-e2e-server.ps1'
$Scanner = Join-Path $Here 'scan-exe-markers.ps1'
$E2eEnv = @('CLASH_ASSIST_E2E_API', 'CLASH_ASSIST_E2E_AUTO_UPDATE', 'CLASH_ASSIST_FAKE_LATEST')
$WorkDirRe = '^[0-9a-f]{32}$'
# 由核心启动的安装程序：%LOCALAPPDATA%\ClashAssist\updates\<32 hex>\setup-<16 hex>.exe
$StagedInstallerRe = '\\ClashAssist\\updates\\[0-9a-f]{32}\\setup-[0-9a-f]{16}\.exe$'

$results = [Collections.Generic.List[object]]::new()
function Add-Result([string]$name, [string]$status, [string]$detail) {
  $results.Add([pscustomobject]@{ case = $name; status = $status; detail = $detail })
  Write-Host "$(if ($status -eq 'pass') { 'ok  ' } else { 'FAIL' }) $name：$detail"
  if ($status -ne 'pass') { Write-Host "::error::$name：$detail" }
}
# 用例脚本块返回失败原因的列表（空 = 通过）；抛异常算失败。Write-Host 的证据行留在日志里。
function Invoke-Case([string]$name, [scriptblock]$body) {
  Write-Host "`n=== $name"
  try {
    $errs = @(& $body | Where-Object { $_ -is [string] -and $_ })
    if ($errs.Count -eq 0) { Add-Result $name 'pass' '通过' } else { Add-Result $name 'fail' ($errs -join '；') }
  } catch {
    Add-Result $name 'fail' "异常：$($_.Exception.Message) @ $($_.InvocationInfo.ScriptLineNumber)"
  }
}

function Get-Dist([string]$dir, [string]$ver) {
  $setup = Join-Path $dir "Clash.Assist_${ver}_x64-setup.exe"
  $zip = Join-Path $dir "Clash.Assist_${ver}_x64-portable.zip"
  foreach ($f in $setup, $zip) { if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { throw "找不到 $f" } }
  @{ Setup = (Resolve-Path -LiteralPath $setup).Path; Zip = (Resolve-Path -LiteralPath $zip).Path; Version = $ver }
}

function Read-UninstallKey { if (Test-Path -LiteralPath $UninstKey) { Get-ItemProperty -LiteralPath $UninstKey } else { $null } }
function Get-Installed {
  $k = Read-UninstallKey
  if (-not $k) { return $null }
  $dir = ([string]$k.InstallLocation).Trim('"')
  $exe = Join-Path $dir ([string]$k.MainBinaryName)
  $pe = if (Test-Path -LiteralPath $exe -PathType Leaf) { (Get-Item -LiteralPath $exe).VersionInfo.ProductVersion } else { $null }
  @{ Version = [string]$k.DisplayVersion; Exe = $exe; Dir = $dir; Pe = $pe }
}
# 安装状态是否正好为 $ver（DisplayVersion 与 PE ProductVersion 都逐字等于三段号；0.3.1.0 不算）
function Test-InstalledIs([string]$ver) { $i = Get-Installed; [bool]($i -and $i.Version -ceq $ver -and $i.Pe -ceq $ver) }
function Format-Installed { $i = Get-Installed; if ($i) { "DisplayVersion=$($i.Version) ProductVersion=$($i.Pe)" } else { '未安装' } }

function Invoke-Setup([string]$setup, [string[]]$argv) {
  $p = Start-Process -FilePath $setup -ArgumentList $argv -Wait -PassThru
  Write-Host "  $(Split-Path -Leaf $setup) $($argv -join ' ') → 退出码 $($p.ExitCode)"
  $p.ExitCode
}

function Get-Descendants([int]$RootPid) {
  $all = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId)
  $out = [Collections.Generic.List[int]]::new(); $q = [Collections.Generic.Queue[int]]::new(); $q.Enqueue($RootPid)
  while ($q.Count) { $p = $q.Dequeue(); foreach ($c in $all | Where-Object { $_.ParentProcessId -eq $p -and $_.ProcessId -ne $p }) { $out.Add([int]$c.ProcessId); $q.Enqueue([int]$c.ProcessId) } }
  $out
}
function Stop-Tree([int]$RootPid) {
  $kids = @(Get-Descendants $RootPid)
  Stop-Process -Id $RootPid -Force -ErrorAction SilentlyContinue
  foreach ($k in $kids) { Stop-Process -Id $k -Force -ErrorAction SilentlyContinue }
}
function Stop-AllApps {
  Get-Process -Name clash-assist -ErrorAction SilentlyContinue | ForEach-Object { Stop-Tree $_.Id }
  Start-Sleep -Seconds 2
}

function Uninstall-All {
  Stop-AllApps
  $i = Get-Installed
  if (-not $i) { return $true }
  $u = Join-Path $i.Dir 'uninstall.exe'
  if (Test-Path -LiteralPath $u) { Start-Process -FilePath $u -ArgumentList '/S' -Wait | Out-Null }
  $deadline = (Get-Date).AddSeconds($UninstallTimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    if (-not (Read-UninstallKey) -and -not (Test-Path -LiteralPath $u)) { return $true }
    Start-Sleep -Seconds 2
  }
  $false
}

# ---------- app.log ----------
function Get-LogLength { if (Test-Path -LiteralPath $AppLog) { (Get-Item -LiteralPath $AppLog).Length } else { 0 } }
# 从偏移 $from 起读取 app.log（应用可能正在写，FileShare.ReadWrite）
function Get-LogSince([long]$from) {
  if (-not (Test-Path -LiteralPath $AppLog)) { return @() }
  $fs = [IO.File]::Open($AppLog, 'Open', 'Read', 'ReadWrite, Delete')
  try {
    if ($from -gt $fs.Length) { $from = 0 }
    [void]$fs.Seek($from, 'Begin')
    $sr = [IO.StreamReader]::new($fs, [Text.UTF8Encoding]::new($false))
    @($sr.ReadToEnd() -split "`r?`n" | Where-Object { $_ })
  } finally { $fs.Dispose() }
}
function Wait-LogLine([long]$from, [string]$pattern, [int]$timeout = $AppSignalTimeoutSeconds) {
  $deadline = (Get-Date).AddSeconds($timeout)
  while ((Get-Date) -lt $deadline) {
    $hit = @(Get-LogSince $from | Where-Object { $_ -match $pattern })
    if ($hit.Count) { return $hit[0] }
    Start-Sleep -Milliseconds 500
  }
  $null
}
function Show-Log([long]$from, [string]$title) {
  Write-Host "  ---- app.log（$title）----"
  Get-LogSince $from | Where-Object { $_ -match '\[update\]|startup' } | ForEach-Object { Write-Host "  | $_" }
}

# ---------- 进程创建审计（Security 4688） ----------
function Get-ProcessCreations([datetime]$since) {
  $ev = @(Get-WinEvent -FilterHashtable @{ LogName = 'Security'; Id = 4688; StartTime = $since } -ErrorAction SilentlyContinue)
  foreach ($e in $ev) {
    $x = [xml]$e.ToXml(); $d = @{}
    foreach ($n in $x.Event.EventData.Data) { $d[$n.Name] = [string]$n.InnerText }
    [pscustomobject]@{
      Time = $e.TimeCreated; Image = $d['NewProcessName']; CommandLine = $d['CommandLine']
      Pid = [Convert]::ToInt32($d['NewProcessId'], 16); ParentPid = [Convert]::ToInt32($d['ProcessId'], 16)
    }
  }
}
function Get-InstallerRuns([datetime]$since) {
  @(Get-ProcessCreations $since | Where-Object { $_.Image -match $StagedInstallerRe -or $_.Image -match '(?i)setup[^\\]*\.exe$' })
}

# ---------- 本地 release 服务器 ----------
function Get-FreePort {
  $l = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
  $l.Start(); $p = ([Net.IPEndPoint]$l.LocalEndpoint).Port; $l.Stop(); $p
}
# 回放并启动服务器：返回 @{ Proc; Port; Log; Api; Sha256; Size }
function Start-Release([string]$name, [string]$tag, [string]$asset, [string]$case = 'Happy') {
  $port = Get-FreePort
  $root = Join-Path $ResultsDir "root-$name"
  & $Server -Replay -Fixture $Fixture -Tag $tag -AssetFile $asset -BaseUrl "http://127.0.0.1:$port" -Case $case -OutDir $root
  if ($LASTEXITCODE -ne 0) { throw "Replay 失败（$name）" }
  $ready = Join-Path $ResultsDir "ready-$name.txt"
  $log = Join-Path $ResultsDir "requests-$name.jsonl"
  $pwsh = (Get-Process -Id $PID).Path
  $p = Start-Process -FilePath $pwsh -PassThru -NoNewWindow -ArgumentList @('-NoProfile', '-File', $Server, '-Serve', '-Root', $root, '-Port', "$port", '-ReadyFile', $ready, '-RequestLog', $log, '-MaxSeconds', '900')
  $t0 = Get-Date
  while (-not (Test-Path -LiteralPath $ready) -and ((Get-Date) - $t0).TotalSeconds -lt 30 -and -not $p.HasExited) { Start-Sleep -Milliseconds 200 }
  if (-not (Test-Path -LiteralPath $ready)) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; throw "服务器未就绪（$name）" }
  @{ Proc = $p; Port = $port; Log = $log; Api = "http://127.0.0.1:$port/release.json"
    Sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $asset).Hash.ToLowerInvariant(); Size = (Get-Item -LiteralPath $asset).Length }
}
function Stop-Release($srv) { if ($srv -and -not $srv.Proc.HasExited) { Stop-Process -Id $srv.Proc.Id -Force -ErrorAction SilentlyContinue } }
function Get-Requests($srv) { if (Test-Path -LiteralPath $srv.Log) { @(Get-Content -LiteralPath $srv.Log | ForEach-Object { $_ | ConvertFrom-Json }) } else { @() } }
function Get-AssetGets($srv) { @(Get-Requests $srv | Where-Object { $_.method -eq 'GET' -and $_.path -match '/releases/download/' }) }

# ---------- 启动应用 ----------
function Start-App([string]$exe, $srv, [switch]$Auto) {
  foreach ($n in $E2eEnv) { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }
  if ($srv) { $env:CLASH_ASSIST_E2E_API = $srv.Api }
  if ($Auto) { $env:CLASH_ASSIST_E2E_AUTO_UPDATE = '1' }
  try { $p = Start-Process -FilePath $exe -WorkingDirectory (Split-Path $exe) -PassThru }
  finally { foreach ($n in $E2eEnv) { Remove-Item "Env:$n" -ErrorAction SilentlyContinue } }
  Write-Host "  启动 $exe（PID $($p.Id)）$(if ($srv) { "API=$($srv.Api)" }) $(if ($Auto) { 'AUTO=1' })"
  $p
}
function Get-WorkDirs { if (Test-Path -LiteralPath $UpdatesDir) { @(Get-ChildItem -LiteralPath $UpdatesDir -Directory -Force | Where-Object { $_.Name -match $WorkDirRe }) } else { @() } }

# 失败 / 拒绝类用例的公共流程：已安装 $from，自动更新应被 $code 拒绝；旧进程保持运行、版本不变、安装程序未运行、无残留
function Invoke-RejectCase([string]$from, [string]$tag, [string]$asset, [string]$case, [string]$expectAuto, [string]$expectCode, [bool]$expectDownload, [string]$expectCheck) {
  $e = @()
  if (-not (Test-InstalledIs $from)) { return @("前置条件：应已安装 $from（实际 $(Format-Installed)）") }
  if (-not $script:AuditOk) { $e += '进程创建审计不可用（audit-control 失败），无法证明安装程序未运行' }
  $srv = Start-Release $script:CurrentCase $tag $asset $case
  $off = Get-LogLength; $t0 = Get-Date; $app = $null
  try {
    $app = Start-App (Get-Installed).Exe $srv -Auto
    $auto = Wait-LogLine $off '\[update\] e2e auto result='
    if (-not $auto) { $e += "等待 $AppSignalTimeoutSeconds 秒没有 e2e auto result 行" }
    elseif ($auto -notmatch $expectAuto) { $e += "e2e auto 结果不符：$auto（期望匹配 $expectAuto）" }
    $lines = Get-LogSince $off
    $check = @($lines | Where-Object { $_ -match '\[update\] check current=' }) | Select-Object -First 1
    if ($expectCheck -and -not ($check -match $expectCheck)) { $e += "check 行不符：$check（期望匹配 $expectCheck）" }
    if ($expectCode -and -not @($lines | Where-Object { $_ -match "\[update\] error code=$expectCode\b" }).Count) { $e += "app.log 中没有 [update] error code=$expectCode" }
    if (@($lines | Where-Object { $_ -match '\[update\] installer launched' }).Count) { $e += '出现了 installer launched 行' }
    Start-Sleep -Seconds 5
    $app.Refresh()
    if ($app.HasExited) { $e += "旧进程退出了（退出码 $($app.ExitCode)），失败时应保持运行" } else { Write-Host "  旧进程 PID $($app.Id) 仍在运行" }
    $gets = @(Get-AssetGets $srv)
    Write-Host "  资产下载请求 $($gets.Count) 次"
    if ($expectDownload -and $gets.Count -ne 1) { $e += "应下载安装包 1 次，实际 $($gets.Count) 次" }
    if (-not $expectDownload -and $gets.Count -ne 0) { $e += "不应下载安装包，实际 $($gets.Count) 次" }
    $runs = @(Get-InstallerRuns $t0)
    if ($runs.Count) { $e += "安装程序被运行了：$(($runs | ForEach-Object { "$($_.Image) [$($_.CommandLine)]" }) -join '; ')" }
    if (Test-Path -LiteralPath $Marker) { $e += "存在 $Marker（不应写入 pending 标记）" }
    $wd = @(Get-WorkDirs)
    if ($wd.Count) { $e += "updates\ 下残留工作目录：$(($wd | ForEach-Object Name) -join ', ')" }
    if (-not (Test-InstalledIs $from)) { $e += "版本变了：$(Format-Installed)" } else { Write-Host "  版本未变：$(Format-Installed)" }
  } finally {
    Show-Log $off $script:CurrentCase
    if ($app) { Stop-Tree $app.Id }
    Stop-AllApps
    Stop-Release $srv
  }
  $e
}
function Invoke-Named([string]$name, [scriptblock]$body) { $script:CurrentCase = $name; Invoke-Case $name $body }

# ---------- 准备 ----------
New-Item -ItemType Directory -Force -Path $ResultsDir | Out-Null
$ResultsDir = (Resolve-Path -LiteralPath $ResultsDir).Path
if (Read-UninstallKey) { Write-Host "::error::本机已安装 $ProductName（$UninstKey 存在），拒绝运行"; exit 2 }
if (Test-Path -LiteralPath $AppDataDir) { Write-Host "::error::$AppDataDir 已存在，拒绝运行"; exit 2 }
foreach ($n in $E2eEnv) { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }
$d0 = Get-Dist $Dist030 '0.3.0'
$d1 = Get-Dist $Dist031 '0.3.1'
$Fixture = (Resolve-Path -LiteralPath $Fixture).Path
$portableDir = Join-Path ([IO.Path]::GetTempPath()) ("ca-e2e-portable-" + [Guid]::NewGuid().ToString('N'))
$script:AuditOk = $false
$script:CurrentCase = ''

try {
  # 正向对照：本 runner 的进程创建审计确实记录了新进程及其命令行（否则「安装程序未运行」的断言无效）
  Invoke-Named 'audit-control' {
    $t0 = (Get-Date).AddSeconds(-1)
    $tok = [Guid]::NewGuid().ToString('N')
    $p = Start-Process -FilePath "$env:SystemRoot\System32\cmd.exe" -ArgumentList @('/c', "exit 0 & rem $tok") -Wait -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 2
    $hit = @(Get-ProcessCreations $t0 | Where-Object { $_.Pid -eq $p.Id -and $_.Image -match '\\cmd\.exe$' })
    if (-not $hit.Count) { return @('Security 日志中没有这次 cmd.exe 的 4688 事件（审计未开启？）') }
    if ($hit[0].CommandLine -notmatch $tok) { return @("4688 事件没有记录命令行（CommandLine=$($hit[0].CommandLine)）") }
    Write-Host "  4688：$($hit[0].Image) [$($hit[0].CommandLine)] 父进程 $($hit[0].ParentPid)"
    $script:AuditOk = $true
  }

  Invoke-Named 'build-versions' {
    $e = @()
    foreach ($d in $d0, $d1) {
      $code = Invoke-Setup $d.Setup @('/S')
      if ($code -ne 0) { $e += "$($d.Version) /S 退出码 $code"; continue }
      Write-Host "  $(Format-Installed)"
      if (-not (Test-InstalledIs $d.Version)) { $e += "$($d.Version)：$(Format-Installed)（ProductVersion 必须逐字为 $($d.Version)）" }
      if (-not (Uninstall-All)) { $e += "$($d.Version) 卸载未完成" }
    }
    $e
  }

  Invoke-Named 'sr4-marker' {
    $e = @()
    $code = Invoke-Setup $d0.Setup @('/S')
    if ($code -ne 0) { return @("0.3.0 /S 退出码 $code") }
    & $Scanner -Exe (Get-Installed).Exe -Build E2E
    if ($LASTEXITCODE -ne 0) { $e += '安装版 exe：-Build E2E 扫描失败' }
    if (-not (Uninstall-All)) { $e += '卸载未完成' }
    Expand-Archive -LiteralPath $d0.Zip -DestinationPath $portableDir
    $pexe = @(Get-ChildItem -LiteralPath $portableDir -Filter '*.exe' -File)
    if ($pexe.Count -ne 1) { $e += "便携 zip 根目录应恰有一个 exe（实际 $($pexe.Count)）" }
    else { & $Scanner -Exe $pexe[0].FullName -Build E2E; if ($LASTEXITCODE -ne 0) { $e += '便携版 exe：-Build E2E 扫描失败' } }
    $e
  }

  Invoke-Named 'server-windows' {
    $e = @()
    foreach ($case in 'Happy', 'WrongDigest', 'WrongSize', 'ImmutableFalse') {
      $srv = Start-Release "server-$case" 'v0.3.1' $d1.Setup $case
      try {
        $j = Invoke-RestMethod -Uri $srv.Api
        $a = @($j.assets)
        $tmp = Join-Path $ResultsDir "dl-$case.exe"
        Invoke-WebRequest -Uri $a[0].browser_download_url -OutFile $tmp
        $got = (Get-FileHash -Algorithm SHA256 -LiteralPath $tmp).Hash.ToLowerInvariant(); Remove-Item -LiteralPath $tmp -Force
        if ($got -ne $srv.Sha256) { $e += "$case：下载内容不一致" }
        if (($a[0].digest -eq "sha256:$($srv.Sha256)") -ne ($case -ne 'WrongDigest')) { $e += "$case：digest=$($a[0].digest)" }
        if (([long]$a[0].size -eq $srv.Size) -ne ($case -ne 'WrongSize')) { $e += "$case：size=$($a[0].size)" }
        if ($j.immutable -isnot [bool] -or $j.immutable -ne ($case -ne 'ImmutableFalse')) { $e += "$case：immutable=$($j.immutable)" }
        if ($a[0].browser_download_url -cne "http://127.0.0.1:$($srv.Port)/JTee77/clash-assist/releases/download/v0.3.1/Clash.Assist_0.3.1_x64-setup.exe") { $e += "$case：URL 形状不符 $($a[0].browser_download_url)" }
      } finally { Stop-Release $srv }
    }
    $e
  }

  # 安装程序侧：已装 0.3.1，直接运行 0.3.0 安装包 /S 与 /S /UPDATE 都必须非 0 且版本不变
  Invoke-Named 'installer-downgrade-rejected' {
    $e = @()
    $code = Invoke-Setup $d1.Setup @('/S'); if ($code -ne 0) { return @("0.3.1 /S 退出码 $code") }
    foreach ($argv in @(, @('/S')) + @(, @('/S', '/UPDATE'))) {
      $code = Invoke-Setup $d0.Setup $argv
      Write-Host "  之后 $(Format-Installed)"
      if ($code -eq 0) { $e += "0.3.0 $($argv -join ' ') 退出码为 0" }
      if (-not (Test-InstalledIs '0.3.1')) {
        $e += "0.3.0 $($argv -join ' ') 之后 $(Format-Installed)（应仍为 0.3.1）"
        [void](Uninstall-All); [void](Invoke-Setup $d1.Setup @('/S'))
      }
    }
    if (-not (Uninstall-All)) { $e += '卸载未完成' }
    $e
  }

  # 便携版：合规的 0.3.1 release 也不能一键更新（S-10）
  Invoke-Named 'portable-one-click' {
    $e = @()
    if (Read-UninstallKey) { return @('前置条件：不应有安装版') }
    if (-not $script:AuditOk) { $e += '进程创建审计不可用，无法证明安装程序未运行' }
    $pexe = @(Get-ChildItem -LiteralPath $portableDir -Filter '*.exe' -File) | Select-Object -First 1 -ExpandProperty FullName
    if (-not $pexe) { return @("$portableDir 中没有便携版 exe") }
    $srv = Start-Release 'portable-one-click' 'v0.3.1' $d1.Setup 'Happy'
    $off = Get-LogLength; $t0 = Get-Date; $app = $null
    try {
      $app = Start-App $pexe $srv -Auto
      $auto = Wait-LogLine $off '\[update\] e2e auto result='
      $check = Wait-LogLine $off '\[update\] check current=' 5
      Write-Host "  $check"; Write-Host "  $auto"
      if ($check -notmatch 'current=0\.3\.0 latest=0\.3\.1 update_available=true install_kind=portable one_click=false reason=portable$') { $e += "check 行不符：$check" }
      if ($auto -notmatch 'result=no_one_click reason=portable$') { $e += "e2e auto 结果不符：$auto" }
      Start-Sleep -Seconds 5; $app.Refresh()
      if ($app.HasExited) { $e += '便携版进程退出了' }
      if (@(Get-AssetGets $srv).Count) { $e += '便携版下载了安装包' }
      $runs = @(Get-InstallerRuns $t0); if ($runs.Count) { $e += "安装程序被运行了：$($runs[0].Image)" }
      if (Test-Path -LiteralPath $Marker) { $e += '写入了 pending 标记' }
      if (Read-UninstallKey) { $e += '出现了安装版' }
    } finally { Show-Log $off 'portable'; if ($app) { Stop-Tree $app.Id }; Stop-AllApps; Stop-Release $srv }
    $e
  }

  # ----- 已装 0.3.0：拒绝类用例 -----
  $code = Invoke-Setup $d0.Setup @('/S')
  if ($code -ne 0) { throw "安装 0.3.0 失败（$code）" }
  # 安装版必须被客户端判为 nsis（S-10）；否则后续一键更新类用例都无意义。这里先记录客户端判定依据的原始状态。
  Invoke-Named 'install-kind-nsis' {
    $e = @()
    $k = Get-Item -LiteralPath $UninstKey
    foreach ($v in 'InstallLocation', 'MainBinaryName', 'DisplayVersion', 'UninstallString') {
      Write-Host ("  {0} = [{1}] {2}" -f $v, $k.GetValueKind($v), $k.GetValue($v, $null, 'DoNotExpandEnvironmentNames'))
    }
    $i = Get-Installed
    $un = Join-Path $i.Dir 'uninstall.exe'
    foreach ($f in $i.Exe, $un, $i.Dir) {
      $it = Get-Item -LiteralPath $f -Force -ErrorAction SilentlyContinue
      Write-Host ("  {0}：{1}" -f $f, $(if ($it) { "Attributes=$($it.Attributes) LinkType=$($it.LinkType)" } else { '不存在' }))
    }
    $srv = Start-Release 'install-kind-nsis' 'v0.3.0' $d0.Setup 'Happy'
    $off = Get-LogLength; $app = $null
    try {
      $app = Start-App $i.Exe $srv
      $check = Wait-LogLine $off '\[update\] check current='
      Write-Host "  $check"
      if ($check -notmatch 'install_kind=nsis ') { $e += "从安装目录启动的 0.3.0 被判为非 nsis：$check" }
    } finally { if ($app) { Stop-Tree $app.Id }; Stop-AllApps; Stop-Release $srv }
    $e
  }
  Invoke-Named 'wrong-digest' { Invoke-RejectCase '0.3.0' 'v0.3.1' $d1.Setup 'WrongDigest' 'result=error step=download' 'digest_mismatch' $true 'one_click=true reason=-$' }
  Invoke-Named 'wrong-size' { Invoke-RejectCase '0.3.0' 'v0.3.1' $d1.Setup 'WrongSize' 'result=error step=download' 'size_mismatch' $true 'one_click=true reason=-$' }
  # C-3：immutable 不是 true → check 阶段就拒绝，不下载
  Invoke-Named 'immutable-false' { Invoke-RejectCase '0.3.0' 'v0.3.1' $d1.Setup 'ImmutableFalse' 'result=no_one_click reason=immutable_false$' '' $false 'update_available=true install_kind=nsis one_click=false reason=immutable_false$' }
  # S-6：JSON 声称 v0.3.2，实际提供 0.3.1 安装包（digest / size 与文件一致）
  Invoke-Named 'pe-mismatch' { Invoke-RejectCase '0.3.0' 'v0.3.2' $d1.Setup 'Happy' 'result=error step=download' 'pe_version_mismatch' $true 'latest=0\.3\.2 update_available=true install_kind=nsis one_click=true reason=-$' }

  # ----- 正常路径：0.3.0 → 0.3.1，走核心的 check → download_and_verify → CreateProcess(/S /UPDATE /R) -----
  Invoke-Named 'happy' {
    $e = @()
    if (-not (Test-InstalledIs '0.3.0')) { return @("前置条件：应已安装 0.3.0（实际 $(Format-Installed)）") }
    if (-not $script:AuditOk) { $e += '进程创建审计不可用，无法证明安装程序由核心经 CreateProcess 启动' }
    $srv = Start-Release 'happy' 'v0.3.1' $d1.Setup 'Happy'
    $off = Get-LogLength; $t0 = (Get-Date).AddSeconds(-1); $app = $null
    try {
      $exe = (Get-Installed).Exe
      $app = Start-App $exe $srv -Auto
      $oldPid = $app.Id
      $launched = Wait-LogLine $off '\[update\] installer launched to=0\.3\.1 fate='
      if (-not $launched) { $e += '没有 installer launched to=0.3.1 行' } else { Write-Host "  $launched" }
      $lines = Get-LogSince $off
      $check = @($lines | Where-Object { $_ -match '\[update\] check current=' }) | Select-Object -First 1
      if ($check -notmatch 'current=0\.3\.0 latest=0\.3\.1 update_available=true install_kind=nsis one_click=true reason=-$') { $e += "check 行不符：$check" }
      if (-not @($lines | Where-Object { $_ -match '\[update\] e2e auto result=launched$' }).Count) { $e += '没有 e2e auto result=launched 行' }
      # 只看旧进程在启动安装程序之前写的行（/R 拉起的 0.3.1 继承了 e2e 环境变量，会再检查一次并记 up_to_date）
      $cut = [array]::FindIndex([string[]]$lines, [Predicate[string]] { param($l) $l -match '\[update\] installer launched' })
      $before = if ($cut -ge 0) { $lines[0..$cut] } else { $lines }
      $errLines = @($before | Where-Object { $_ -match '\[update\] error code=' })
      if ($errLines.Count) { $e += "启动安装程序之前出现了 error 行：$($errLines -join ' / ')" }
      # 旧进程退出
      $deadline = (Get-Date).AddSeconds(30)
      while (-not $app.HasExited -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500; $app.Refresh() }
      if (-not $app.HasExited) { $e += '旧进程 30 秒内没有退出' } else { Write-Host "  旧进程 PID $oldPid 已退出（退出码 $($app.ExitCode)）" }
      # 安装完成 + 新进程（/R）
      $deadline = (Get-Date).AddSeconds(120); $new = $null
      while ((Get-Date) -lt $deadline) {
        $new = Get-Process -Name clash-assist -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $oldPid -and $_.Path -and $_.Path -ieq $exe } | Select-Object -First 1
        if ($new -and (Test-InstalledIs '0.3.1')) { break }
        Start-Sleep -Seconds 1
      }
      Write-Host "  安装后 $(Format-Installed)；新进程 $(if ($new) { "PID $($new.Id)" } else { '无' })"
      if (-not (Test-InstalledIs '0.3.1')) { $e += "安装后应为 0.3.1：$(Format-Installed)" }
      if (-not $new) { $e += '安装后没有新的 clash-assist.exe 进程（/R）' }
      elseif ((Get-Item -LiteralPath $new.Path).VersionInfo.ProductVersion -cne '0.3.1') { $e += "新进程 ProductVersion 不是 0.3.1" }
      # 服务器：安装包恰好下载 1 次、字节数与 size 一致
      $gets = @(Get-AssetGets $srv)
      if ($gets.Count -ne 1 -or $gets[0].status -ne 200 -or [long]$gets[0].bytes -ne $srv.Size) { $e += "安装包下载记录不符：$($gets | ConvertTo-Json -Compress)" }
      # 4688：安装程序是核心暂存的那个文件，由旧进程直接 CreateProcess，参数 /S /UPDATE /R
      Start-Sleep -Seconds 2
      $runs = @(Get-InstallerRuns $t0)
      foreach ($r in $runs) { Write-Host "  4688：$($r.Image) [$($r.CommandLine)] 父进程 $($r.ParentPid)" }
      $staged = @($runs | Where-Object { $_.Image -match $StagedInstallerRe })
      if ($staged.Count -ne 1) { $e += "应恰有 1 次从 updates\<32hex>\setup-<16hex>.exe 启动的安装程序，实际 $($staged.Count) 次" }
      elseif ($staged[0].ParentPid -ne $oldPid) { $e += "安装程序的父进程是 $($staged[0].ParentPid)，不是旧应用进程 $oldPid" }
      elseif ($staged[0].CommandLine -notmatch '\s/S\s+/UPDATE\s+/R\s*$') { $e += "安装程序命令行不符：$($staged[0].CommandLine)" }
      if ($runs.Count -ne $staged.Count) { $e += "还运行了其他安装程序：$(($runs | Where-Object { $_.Image -notmatch $StagedInstallerRe } | ForEach-Object Image) -join ', ')" }
      # pending：新进程启动时对账 → updated，标记删除
      $pend = Wait-LogLine $off '\[update\] pending outcome=' 60
      Write-Host "  $pend"
      if ($pend -notmatch 'pending outcome=updated from=0\.3\.0 to=0\.3\.1$') { $e += "pending 对账行不符：$pend" }
      Start-Sleep -Seconds 3
      if (Test-Path -LiteralPath $Marker) { $e += "对账后 $Marker 仍存在" }
      # 再启动一次（不带 e2e 环境变量）：不应再提示，遗留工作目录被清理
      if ($new) { Stop-Tree $new.Id }
      Stop-AllApps
      $off2 = Get-LogLength
      $again = Start-App $exe $null
      $startup = Wait-LogLine $off2 'startup frontend_url=' 60
      Start-Sleep -Seconds 8
      $l2 = Get-LogSince $off2
      Show-Log $off2 'happy 第三次启动'
      if (-not $startup) { $e += '第三次启动没有 startup 行' }
      if (@($l2 | Where-Object { $_ -match '\[update\] pending outcome=' }).Count) { $e += "第三次启动又出现 pending 对账行：$(@($l2 | Where-Object { $_ -match 'pending outcome=' }) -join ' / ')" }
      $all = Get-LogSince $off
      $upd = @($all | Where-Object { $_ -match '\[update\] pending outcome=updated' }).Count
      if ($upd -ne 1) { $e += "整个用例中 pending outcome=updated 出现 $upd 次（应为 1 次）" }
      if (Test-Path -LiteralPath $Marker) { $e += "第三次启动后 $Marker 仍存在" }
      $wd = @(Get-WorkDirs); if ($wd.Count) { $e += "第三次启动后 updates\ 仍有工作目录：$(($wd | ForEach-Object Name) -join ', ')" }
      Stop-Tree $again.Id
    } finally { Show-Log $off 'happy'; Stop-AllApps; Stop-Release $srv }
    $e
  }

  # ----- 已装 0.3.1：降级与 semver -----
  if (-not (Test-InstalledIs '0.3.1')) {
    Write-Host "  happy 之后不是 0.3.1（$(Format-Installed)），重新安装 0.3.1 以继续后续用例"
    [void](Uninstall-All); Remove-Item -LiteralPath $AppDataDir -Recurse -Force -ErrorAction SilentlyContinue
    if ((Invoke-Setup $d1.Setup @('/S')) -ne 0) { throw '安装 0.3.1 失败' }
  }
  # JSON 声称 v0.3.2，实际提供 0.3.0 安装包：PE 校验拒绝，安装程序不运行
  Invoke-Named 'downgrade' { Invoke-RejectCase '0.3.1' 'v0.3.2' $d0.Setup 'Happy' 'result=error step=download' 'pe_version_mismatch' $true 'current=0\.3\.1 latest=0\.3\.2 update_available=true install_kind=nsis one_click=true reason=-$' }
  # semver：合规的 v0.3.0 release（不比当前新）→ check 阶段 up_to_date，不下载
  Invoke-Named 'semver-not-newer' { Invoke-RejectCase '0.3.1' 'v0.3.0' $d0.Setup 'Happy' 'result=no_one_click reason=up_to_date$' '' $false 'current=0\.3\.1 latest=0\.3\.0 update_available=false install_kind=nsis one_click=false reason=up_to_date$' }
  # semver：同版本 v0.3.1 → up_to_date
  Invoke-Named 'semver-same-version' { Invoke-RejectCase '0.3.1' 'v0.3.1' $d1.Setup 'Happy' 'result=no_one_click reason=up_to_date$' '' $false 'current=0\.3\.1 latest=0\.3\.1 update_available=false install_kind=nsis one_click=false reason=up_to_date$' }
} catch {
  Add-Result 'harness' 'fail' "用例框架异常：$($_.Exception.Message) @ $($_.InvocationInfo.ScriptLineNumber)"
} finally {
  Write-Host "`n=== 清理"
  if (Test-Path -LiteralPath $AppLog) { Copy-Item -LiteralPath $AppLog -Destination (Join-Path $ResultsDir 'app.log') -Force -ErrorAction SilentlyContinue }
  [void](Uninstall-All)
  Remove-Item -LiteralPath $portableDir -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $AppDataDir -Recurse -Force -ErrorAction SilentlyContinue
  $results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $ResultsDir 'results.json') -Encoding utf8
  if ($env:GITHUB_STEP_SUMMARY) {
    $md = @('### updater-e2e 用例', '', '| 用例 | 结果 | 说明 |', '|---|---|---|')
    foreach ($r in $results) { $md += "| $($r.case) | $($r.status) | $($r.detail -replace '\|', '/') |" }
    ($md -join "`n") | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Append -Encoding utf8
  }
}

$fail = @($results | Where-Object status -ne 'pass').Count
$pass = @($results | Where-Object status -eq 'pass').Count
Write-Host "`nupdater-e2e：通过 $pass，失败 $fail"
exit $(if ($fail) { 1 } else { 0 })
