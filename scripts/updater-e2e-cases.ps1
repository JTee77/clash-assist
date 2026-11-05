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
  另有一项 layered（frontend-fallback-contract，前端回退契约）：明确标注「不在无界面 e2e 中验收」，
  既不计为通过也不计为失败，由 CI 的 vitest / cargo test 分层验收（docs/windows-updater.md）。
  已有 Clash Assist 安装或 %LOCALAPPDATA%\ClashAssist 时拒绝运行。

  退出码：0 = 全部通过；1 = 有失败；2 = 用法或环境错误。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)][string]$Dist030,
  [Parameter(Mandatory)][string]$Dist031,
  [Parameter(Mandatory)][string]$Fixture,
  # scripts/updater-e2e-exit-nonzero.nsi 编译出的测试安装程序（ProductVersion 0.3.1，启动即以 42 退出）
  [Parameter(Mandatory)][string]$ExitNonzeroInstaller,
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
# 测试安装程序的唯一标记（版本资源中的 UTF-16LE 字符串）；release 产物中不得出现
$TestInstallerMarker = 'CLASH_ASSIST_E2E_TEST_INSTALLER_EXIT_NONZERO'
$TestInstallerExitCode = 42

$results = [Collections.Generic.List[object]]::new()
function Add-Result([string]$name, [string]$status, [string]$detail) {
  $results.Add([pscustomobject]@{ case = $name; status = $status; detail = $detail })
  $tag = switch ($status) { 'pass' { 'ok  ' } 'layered' { 'LAYR' } default { 'FAIL' } }
  Write-Host "$tag $name：$detail"
  if ($status -eq 'layered') { Write-Host "::notice::$name：$detail" }
  elseif ($status -ne 'pass') { Write-Host "::error::$name：$detail" }
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
# 进程终止审计（Security 4689）：退出码与时刻
function Get-ProcessTerminations([datetime]$since) {
  $ev = @(Get-WinEvent -FilterHashtable @{ LogName = 'Security'; Id = 4689; StartTime = $since } -ErrorAction SilentlyContinue)
  foreach ($e in $ev) {
    $x = [xml]$e.ToXml(); $d = @{}
    foreach ($n in $x.Event.EventData.Data) { $d[$n.Name] = [string]$n.InnerText }
    [pscustomobject]@{
      Time = $e.TimeCreated; Image = $d['ProcessName']
      Pid = [Convert]::ToInt32($d['ProcessId'], 16); Status = [Convert]::ToInt64($d['Status'], 16)
    }
  }
}
# 与核心 install::launch_log_path 相同的规范化：只去掉 \\?\ / \\?\UNC\ 前缀，其余逐字节保留
function ConvertTo-LaunchPath([string]$p) {
  if ($p.StartsWith('\\?\UNC\', [StringComparison]::Ordinal)) { return '\\' + $p.Substring(8) }
  if ($p.StartsWith('\\?\', [StringComparison]::Ordinal)) { return $p.Substring(4) }
  $p
}
# [update] launch_attempt pid=<pid> to=<ver> path=<path>
function Get-LaunchAttempts([string[]]$lines) {
  foreach ($l in $lines) {
    if ($l -cmatch '\[update\] launch_attempt pid=(\d+) to=(\S+) path=(.+)$') {
      [pscustomobject]@{ Pid = [int]$Matches[1]; To = $Matches[2]; Path = $Matches[3]; Line = $l }
    }
  }
}
# 文件中某字符串的出现次数（UTF-8 与 UTF-16LE 两种编码）
function Get-MarkerCount([string]$file, [string]$text) {
  # Latin-1 把每个字节一一映射成一个字符，于是可以用序数字符串查找做字节串匹配
  $l1 = [Text.Encoding]::Latin1
  $hay = $l1.GetString([IO.File]::ReadAllBytes($file)); $n = 0
  foreach ($enc in [Text.Encoding]::UTF8, [Text.Encoding]::Unicode) {
    $pat = $l1.GetString($enc.GetBytes($text)); $i = 0
    while (($i = $hay.IndexOf($pat, $i, [StringComparison]::Ordinal)) -ge 0) { $n++; $i += $pat.Length }
  }
  $n
}
# 安装目录快照：相对路径、大小、修改时间、SHA-256
function Get-DirSnapshot([string]$dir) {
  @(Get-ChildItem -LiteralPath $dir -Recurse -Force -File | Sort-Object FullName | ForEach-Object {
    "$($_.FullName.Substring($dir.Length))|$($_.Length)|$($_.LastWriteTimeUtc.Ticks)|$((Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash)"
  }) -join "`n"
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
    if (@(Get-LaunchAttempts $lines).Count) { $e += "出现了 launch_attempt 行（不应走到 CreateProcess）：$(@(Get-LaunchAttempts $lines)[0].Line)" }
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
    # 进程终止审计（4689）的正向对照：退出码 5 必须被记录
    $p2 = Start-Process -FilePath "$env:SystemRoot\System32\cmd.exe" -ArgumentList @('/c', "exit 5 & rem $tok") -Wait -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 2
    $term = @(Get-ProcessTerminations $t0 | Where-Object { $_.Pid -eq $p2.Id -and $_.Image -match '\\cmd\.exe$' })
    if (-not $term.Count) { return @('Security 日志中没有 cmd.exe 的 4689 事件（进程终止审计未开启？）') }
    if ($term[0].Status -ne 5) { return @("4689 记录的退出码是 $($term[0].Status)，应为 5") }
    Write-Host "  4689：$($term[0].Image) PID $($term[0].Pid) 退出码 $($term[0].Status)"
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

  # 测试安装程序与 release 产物隔离：标记只在测试安装程序里；它的 ProductVersion 逐字为 0.3.1
  Invoke-Named 'test-installer-isolation' {
    $e = @()
    $n = Get-MarkerCount $ExitNonzeroInstaller $TestInstallerMarker
    Write-Host "  测试安装程序 $ExitNonzeroInstaller：标记 $n 处"
    if ($n -lt 1) { $e += '测试安装程序中没有标记' }
    $pv = (Get-Item -LiteralPath $ExitNonzeroInstaller).VersionInfo.ProductVersion
    if ($pv -cne '0.3.1') { $e += "测试安装程序 ProductVersion=$pv（应逐字为 0.3.1）" }
    $files = @($d0.Setup, $d1.Setup) + @(Get-ChildItem -LiteralPath $portableDir -Filter '*.exe' -File | ForEach-Object FullName)
    foreach ($zip in $d0.Zip, $d1.Zip) {
      $x = Join-Path $ResultsDir ("iso-" + [Guid]::NewGuid().ToString('N')); Expand-Archive -LiteralPath $zip -DestinationPath $x
      $files += @(Get-ChildItem -LiteralPath $x -Recurse -File | ForEach-Object FullName)
    }
    foreach ($f in $files) {
      $c = Get-MarkerCount $f $TestInstallerMarker
      Write-Host "  $f：标记 $c 处"
      if ($c) { $e += "release 形态产物含测试标记：$f" }
    }
    if ((Get-FileHash -LiteralPath $ExitNonzeroInstaller).Hash -in @((Get-FileHash -LiteralPath $d0.Setup).Hash, (Get-FileHash -LiteralPath $d1.Setup).Hash)) { $e += '测试安装程序与真实安装包相同' }
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

  # 经应用完整路径（下载、校验、持句柄 CreateProcess）启动一个立即以 42 退出的测试安装程序（ProductVersion 0.3.1，
  # size / digest 为真实值）：核心应在 2 s 观察期内看到非零退出，报 installer_exit_nonzero，清理标记与工作目录，旧进程不退出。
  Invoke-Named 'installer-exit-nonzero' {
    $e = @()
    if (-not (Test-InstalledIs '0.3.0')) { return @("前置条件：应已安装 0.3.0（实际 $(Format-Installed)）") }
    if (-not $script:AuditOk) { $e += '进程审计不可用，无法证明测试安装程序被创建并以非零码退出' }
    $inst = Get-Installed
    $snap0 = Get-DirSnapshot $inst.Dir
    $srv = Start-Release 'installer-exit-nonzero' 'v0.3.1' $ExitNonzeroInstaller 'Happy'
    $off = Get-LogLength; $t0 = (Get-Date).AddSeconds(-1); $app = $null
    try {
      $app = Start-App $inst.Exe $srv -Auto
      $oldPid = $app.Id
      $auto = Wait-LogLine $off '\[update\] e2e auto result=error step=launch'
      Write-Host "  $auto"
      if (-not $auto) { $e += '没有 e2e auto result=error step=launch 行' }
      elseif ($auto -notmatch '退出码 42') { $e += "launch 错误说明里没有退出码 42：$auto" }
      $lines = Get-LogSince $off
      $check = @($lines | Where-Object { $_ -match '\[update\] check current=' }) | Select-Object -First 1
      if ($check -notmatch 'current=0\.3\.0 latest=0\.3\.1 update_available=true install_kind=nsis one_click=true reason=-$') { $e += "check 行不符：$check" }
      $codes = @($lines | Where-Object { $_ -match '\[update\] error code=' })
      if ($codes.Count -ne 1 -or $codes[0] -notmatch '\[update\] error code=installer_exit_nonzero ') { $e += "错误码行应恰为一行 installer_exit_nonzero：$($codes -join ' / ')" }
      elseif ($auto) {
        # relaunch_updated_app 返回给调用方（e2e 触发器 / 前端 invoke）的字符串 = error code 行的说明文字（分层契约的 e2e 一侧）
        $returned = ($auto -split 'e2e auto result=error step=launch ', 2)[1]
        $logged = ($codes[0] -split 'error code=installer_exit_nonzero ', 2)[1]
        if ($returned -cne $logged) { $e += "返回给调用方的错误与 error code 行不一致：[$returned] ≠ [$logged]" } else { Write-Host "  返回给调用方的错误：$returned" }
      }
      if (-not @($lines | Where-Object { $_ -cmatch '\[update\] 安装程序：ExitedErr\(Some\(42\)\)$' }).Count) { $e += '没有「安装程序：ExitedErr(Some(42))」行' }
      if (@($lines | Where-Object { $_ -match '\[update\] installer launched' }).Count) { $e += '出现了 installer launched 行' }
      $att = @(Get-LaunchAttempts $lines)
      if ($att.Count -ne 1) { $e += "launch_attempt 行应恰有 1 行，实际 $($att.Count)" }
      elseif ($att[0].Pid -ne $oldPid -or $att[0].To -cne '0.3.1') { $e += "launch_attempt 不符：$($att[0].Line)（旧 PID $oldPid）" }
      # 旧进程保持运行
      Start-Sleep -Seconds 5; $app.Refresh()
      if ($app.HasExited) { $e += "旧进程退出了（退出码 $($app.ExitCode)）" } else { Write-Host "  旧进程 PID $oldPid 仍在运行" }
      # 4688 / 4689：恰好一次创建，父进程 = 旧进程，路径与 launch_attempt 逐字节一致，参数完整；2 s 内以 42 退出
      Start-Sleep -Seconds 1
      $runs = @(Get-InstallerRuns $t0)
      foreach ($r in $runs) { Write-Host "  4688：$($r.Image) [$($r.CommandLine)] 父进程 $($r.ParentPid) PID $($r.Pid)" }
      if ($runs.Count -ne 1) { $e += "应恰有 1 次安装程序创建，实际 $($runs.Count)" }
      else {
        $r = $runs[0]; $img = ConvertTo-LaunchPath $r.Image
        if ($r.ParentPid -ne $oldPid) { $e += "父进程 $($r.ParentPid) 不是旧进程 $oldPid" }
        if ($img -cnotmatch ('^' + [regex]::Escape((Join-Path $env:LOCALAPPDATA 'ClashAssist\updates')) + '\\[0-9a-f]{32}\\setup-[0-9a-f]{16}\.exe$')) { $e += "映像不在 updates\<32hex>\ 下：$img" }
        if ($att.Count -eq 1 -and $img -cne $att[0].Path) { $e += "4688 映像与 launch_attempt 路径不一致：[$img] ≠ [$($att[0].Path)]" }
        if ($r.CommandLine -cne ('"' + $r.Image + '" /S /UPDATE /R')) { $e += "命令行不符：$($r.CommandLine)" }
        $term = @(Get-ProcessTerminations $t0 | Where-Object { $_.Pid -eq $r.Pid -and $_.Time -ge $r.Time }) | Select-Object -First 1
        if (-not $term) { $e += '没有测试安装程序的 4689 终止事件' }
        else {
          $dt = ($term.Time - $r.Time).TotalMilliseconds
          Write-Host "  4689：PID $($term.Pid) 退出码 $($term.Status)，存活 $([int]$dt) ms"
          if ($term.Status -ne $TestInstallerExitCode) { $e += "4689 退出码 $($term.Status)，应为 $TestInstallerExitCode" }
          if ($dt -ge 2000) { $e += "测试安装程序存活 $([int]$dt) ms，不在 2 s 观察期内" }
        }
      }
      $gets = @(Get-AssetGets $srv)
      if ($gets.Count -ne 1 -or $gets[0].status -ne 200 -or [long]$gets[0].bytes -ne $srv.Size) { $e += "安装包下载记录不符：$($gets | ConvertTo-Json -Compress)" }
      if (Test-Path -LiteralPath $Marker) { $e += "存在 $Marker（失败后应当场删除）" }
      $wd = @(Get-WorkDirs); if ($wd.Count) { $e += "updates\ 下残留工作目录：$(($wd | ForEach-Object Name) -join ', ')" }
      if (-not (Test-InstalledIs '0.3.0')) { $e += "版本变了：$(Format-Installed)" } else { Write-Host "  版本未变：$(Format-Installed)" }
      if ((Get-DirSnapshot $inst.Dir) -cne $snap0) { $e += '安装目录内容被改动了' } else { Write-Host '  安装目录内容未变' }
    } finally { Show-Log $off 'installer-exit-nonzero'; if ($app) { Stop-Tree $app.Id }; Stop-AllApps; Stop-Release $srv }
    $e
  }
  # 前端回退契约（自动化分层测试）：无界面 e2e 不经前端，这里不验收、也不计为通过（状态 layered）。
  # 分层证据见 docs/windows-updater.md「前端回退契约（自动化分层测试）」：
  #   - 本用例 installer-exit-nonzero：生产启动路径把 installer_exit_nonzero 的说明文字原样交给调用方（见上方断言）；
  #   - Rust 单测：relaunch_error_reaches_frontend_as_plain_string、relaunch_command_returns_string_error、
  #     launch_blocking_nonzero_exit_maps_to_frontend_error_and_cleans_up（Windows）——命令以 JSON 字符串 reject；
  #   - vitest：src/hooks/useUpdateFlow.fallback.test.ts——真实 useUpdateFlow 恰好打开一次发布页、停在 fallback；
  #   - 真实浏览器被打开：M6 实机测试。
  $script:CurrentCase = 'frontend-fallback-contract'
  Add-Result 'frontend-fallback-contract' 'layered' '前端回退契约（自动化分层测试）：不在无界面 e2e 中验收，不计为通过；由 CI 的 vitest（useUpdateFlow.fallback.test.ts）与 cargo test（relaunch_* / launch_blocking_nonzero_exit_*）验收，真实浏览器打开留给 M6 实机测试'

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
      $autoLine = Wait-LogLine $off '\[update\] e2e auto result='
      Write-Host "  $autoLine"
      if ($autoLine -notmatch 'e2e auto result=launched$') { $e += "e2e auto 结果不是 launched：$autoLine" }
      $lines = Get-LogSince $off
      $check = @($lines | Where-Object { $_ -match '\[update\] check current=' }) | Select-Object -First 1
      if ($check -notmatch 'current=0\.3\.0 latest=0\.3\.1 update_available=true install_kind=nsis one_click=true reason=-$') { $e += "check 行不符：$check" }
      # 只看旧进程在启动安装程序之前写的行（/R 拉起的 0.3.1 继承了 e2e 环境变量，会再检查一次并记 up_to_date）
      $cut = [array]::FindIndex([string[]]$lines, [Predicate[string]] { param($l) $l -match '\[update\] e2e auto result=' })
      $before = if ($cut -ge 0) { $lines[0..$cut] } else { $lines }
      $errLines = @($before | Where-Object { $_ -match '\[update\] error code=' })
      if ($errLines.Count) { $e += "启动安装程序之前出现了 error 行：$($errLines -join ' / ')" }
      # 旧进程退出
      $deadline = (Get-Date).AddSeconds(30)
      while (-not $app.HasExited -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500; $app.Refresh() }
      if (-not $app.HasExited) { $e += '旧进程 30 秒内没有退出' } else { Write-Host "  旧进程 PID $oldPid 已退出（退出码 $($app.ExitCode)，退出时刻 $($app.ExitTime.ToString('o'))）" }
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
      $lines = Get-LogSince $off
      $runs = @(Get-InstallerRuns $t0)
      foreach ($r in $runs) { Write-Host "  4688：$($r.Image) [$($r.CommandLine)] 父进程 $($r.ParentPid)" }
      $staged = @($runs | Where-Object { $_.Image -match $StagedInstallerRe })
      if ($staged.Count -ne 1) { $e += "应恰有 1 次从 updates\<32hex>\setup-<16hex>.exe 启动的安装程序，实际 $($staged.Count) 次" }
      elseif ($staged[0].ParentPid -ne $oldPid) { $e += "安装程序的父进程是 $($staged[0].ParentPid)，不是旧应用进程 $oldPid" }
      elseif ($staged[0].Image -cnotmatch ('^' + [regex]::Escape((Join-Path $env:LOCALAPPDATA 'ClashAssist\updates')) + '\\[0-9a-f]{32}\\setup-[0-9a-f]{16}\.exe$')) { $e += "安装程序映像不在 %LOCALAPPDATA%\ClashAssist\updates\<32hex>\ 下：$($staged[0].Image)" }
      # Rust std 的 CreateProcess 命令行：程序路径加引号，三个参数原样，不多不少
      elseif (@(Get-LaunchAttempts $lines).Count -ne 1) { $e += "launch_attempt 行应恰有 1 行：$(@(Get-LaunchAttempts $lines).Line -join ' / ')" }
      elseif (@(Get-LaunchAttempts $lines)[0].Pid -ne $oldPid -or @(Get-LaunchAttempts $lines)[0].To -cne '0.3.1') { $e += "launch_attempt 不符：$(@(Get-LaunchAttempts $lines)[0].Line)（旧 PID $oldPid）" }
      elseif ((ConvertTo-LaunchPath $staged[0].Image) -cne @(Get-LaunchAttempts $lines)[0].Path) { $e += "4688 映像与 launch_attempt 路径不一致：[$(ConvertTo-LaunchPath $staged[0].Image)] ≠ [$(@(Get-LaunchAttempts $lines)[0].Path)]" }
      elseif ($staged[0].CommandLine -cne ('"' + $staged[0].Image + '" /S /UPDATE /R')) { $e += "安装程序命令行不是 `"<暂存路径>`" /S /UPDATE /R：$($staged[0].CommandLine)" }
      elseif (-not $app.HasExited -or $app.ExitTime -le $staged[0].Time) { $e += "旧进程没有在安装程序启动之后退出（安装程序 $($staged[0].Time.ToString('o'))，旧进程退出 $(if ($app.HasExited) { $app.ExitTime.ToString('o') } else { '未退出' })）" }
      # 证据约定见 docs/windows-updater.md「成功路径的证据」：`installer launched` 行在观察期结束后才写，
      # 而安装程序（Section Install 的 CheckIfAppIsRunning）可能在观察期内结束旧进程，所以这一行不保证出现，只记录。
      # 成功以 4688（父进程 = 旧进程、暂存路径、完整 /S /UPDATE /R、启动早于旧进程退出）、精确 0.3.1、新进程、
      # pending 对账恰好一次与清理为准。
      $launchedLine = @($lines | Where-Object { $_ -match '\[update\] installer launched to=' }) | Select-Object -First 1
      Write-Host "  installer launched 行：$(if ($launchedLine) { $launchedLine } else { '缺失（旧进程在观察期内被安装程序结束）' })"

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
    $md = @('### updater-e2e 用例', '', '`layered` = 不在本 e2e 中验收、不计为通过，由 CI 的 vitest / cargo test 分层验收。', '', '| 用例 | 结果 | 说明 |', '|---|---|---|')
    foreach ($r in $results) { $md += "| $($r.case) | $($r.status) | $($r.detail -replace '\|', '/') |" }
    ($md -join "`n") | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Append -Encoding utf8
  }
}

$fail = @($results | Where-Object { $_.status -notin 'pass', 'layered' }).Count
$pass = @($results | Where-Object status -eq 'pass').Count
$layered = @($results | Where-Object status -eq 'layered').Count
Write-Host "`nupdater-e2e：通过 $pass，失败 $fail，分层验收（不在此计为通过）$layered"
exit $(if ($fail) { 1 } else { 0 })
