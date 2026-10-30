<#
.SYNOPSIS
  M1 跨用户回放（Ellie V5 复跑复核 §2-4 MUST-4、§2-11）：S-D′（mihomo 默认 SDDL + 他人第二实例）
  与规则 4 的 token-mismatch / unverified。由 .github/workflows/ci.yml 的 xuser-windows job 调用。

.DESCRIPTION
  全部进程都以标准用户 ca_a / ca_b 运行（scripts/ci-standard-users.ps1，口令只在内存 / DPAPI 中）。
  客户端是 M1 的生产代码：src-tauri/src/mihomo/xuser_replay_tests.rs 中两个 #[ignore] 测试，
  由 -ClientExe（cargo test --no-run 产出的 clash_assist_lib 测试二进制）以 ca_a 身份运行。

  S-D′（全部经 ScheduledTask 启动，各自独立 logon，避开 seclogon 共享 logon SID 的假象）：
    1. ca_a 用 CreateNamedPipeW 按 mihomo 默认 SDDL `D:P(A;OICI;0x12019f;;;BU)(A;OICI;0x12019f;;;SY)`
       创建 \\.\pipe\verge-mihomo-sidecar-release-<ca_a 的 ownerKey>（属主 = ca_a，与 V5 中真实
       mihomo 的属主 / DACL 一致）。不用真实 mihomo：它无法统计收到的字节；也不用 .NET
       PipeSecurity：它会把非容器对象 ACE 上的 OICI 规范化掉，与 go-winio 实际写入的 SD 不同。
    2. ca_b 用 .NET NamedPipeServerStream（MaxAllowedServerInstances、无 FirstPipeInstance）挂第二实例。
    3. ca_a 运行客户端连 N 次。断言：每次 InsecureDacl / dacl-allows-foreign-instance；ca_b 的实例
       确实接受过连接（accepted=true，Ellie M-c：超时不能算 0 字节通过）；两个实例收到的字节数都为 0。
  规则 4（ca_b 运行 verge-mihomo.exe -ext-ctl 127.0.0.1:<port>，ca_a 运行客户端，两边同一种启动方式）：
    Credential↔Credential ⇒ token-mismatch（令牌用户 = ca_b）；ScheduledTask↔ScheduledTask ⇒ unverified。

  结果写到 <ResultsDir>\results.json（assertions + evidence），各作业的输出文件复制到同目录下；
  不复制 spec.json 与 ci-standard-users 状态目录（含 DPAPI 凭据）。本回放不使用任何 secret。
  finally 中停止全部作业、结束残留进程并删除用户 / 计划任务（workflow 另有 if: always() 兜底）。

.PARAMETER ClientExe
  clash_assist_lib 测试二进制（含 xuser_replay_tests）。

.PARAMETER ResultsDir
  结果目录（artifact）。不得位于 ci-standard-users 的状态目录内。

.PARAMETER WorkDir
  下载与暂存目录，默认 $env:RUNNER_TEMP\m1-xuser。

.NOTES
  需要管理员权限（runner 默认满足）；在 PowerShell 7（pwsh）下运行。
  退出码：0 = 全部断言通过；1 = 有断言失败或步骤出错。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$ClientExe,

  [Parameter(Mandatory = $true)]
  [string]$ResultsDir,

  [string]$WorkDir
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# 与 win/v5-pipe-spike 相同的 mihomo（sha256 取自 GitHub release 资产 digest）
$MihomoVersion = 'v1.19.33'
$MihomoAsset = "mihomo-windows-amd64-compatible-$MihomoVersion.zip"
$MihomoUrl = "https://github.com/MetaCubeX/mihomo/releases/download/$MihomoVersion/$MihomoAsset"
$MihomoSha256 = '7aaf636a4f2172dcd8ec34ac4f52797b059195e5afd6409508c963e545f8864d'

# mihomo 未设 LISTEN_NAMEDPIPE_SDDL 时管道上实际的 DACL（V5 S-D′ 读回值）
$MihomoDefaultSddl = 'D:P(A;OICI;0x12019f;;;BU)(A;OICI;0x12019f;;;SY)'
$SidAdmins = 'S-1-5-32-544'
$PipePrefix = '\\.\pipe\'
$PortCredential = 19094
$PortTask = 19093
$PipeConnects = 8
$ReadyWaitSec = 60
$ClientTimeoutSec = 180
$ClientTestPipe = 'mihomo::xuser_replay_tests::xuser_s_d_prime_client'
$ClientTestTcp = 'mihomo::xuser_replay_tests::xuser_tcp_listener_check'

$base = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
if (-not $WorkDir) { $WorkDir = Join-Path $base 'm1-xuser' }
$StateDir = Join-Path $base 'ca-standard-users'   # 与 ci-standard-users.ps1 默认值相同
$UsersScript = Join-Path $PSScriptRoot 'ci-standard-users.ps1'
$StageDir = Join-Path $WorkDir 'stage'
$MihomoExe = Join-Path $WorkDir 'bin\verge-mihomo.exe'
$ClientName = 'ca-m1-client.exe'
$PwshWin = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'

$script:Asserts = New-Object System.Collections.Generic.List[object]
$script:Errors = New-Object System.Collections.Generic.List[string]
$script:Started = New-Object System.Collections.Generic.List[string]
$script:Users = @{}

function Write-Info([string]$Message) { Write-Host "[m1-xuser] $Message" }

function Add-Error([string]$Message) {
  $script:Errors.Add($Message)
  Write-Warning $Message
}

function Add-Assert([string]$Name, [bool]$Pass, $Expected, $Actual) {
  $script:Asserts.Add([ordered]@{ name = $Name; pass = $Pass; expected = $Expected; actual = $Actual })
  $mark = if ($Pass) { 'PASS' } else { 'FAIL' }
  Write-Info "$mark $Name（期望 $Expected，实际 $Actual）"
}

function Write-Json([string]$Path, $Value) {
  $dir = Split-Path -Parent $Path
  if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
  ConvertTo-Json -InputObject $Value -Depth 16 | Set-Content -LiteralPath $Path -Encoding UTF8
}

function Get-Sha256Hex([byte[]]$Bytes) {
  $sha = [Security.Cryptography.SHA256]::Create()
  try { $hash = $sha.ComputeHash($Bytes) } finally { $sha.Dispose() }
  return (($hash | ForEach-Object { $_.ToString('x2') }) -join '')
}

function Read-SharedText([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path)) { return $null }
  $fs = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
  try {
    $sr = New-Object IO.StreamReader($fs)
    try { return $sr.ReadToEnd() } finally { $sr.Dispose() }
  } finally { $fs.Dispose() }
}

# StrictMode 下读不存在的属性会抛错；结果 JSON 各变体字段不同
function Get-Prop($Obj, [string]$Name) {
  if ($null -eq $Obj) { return $null }
  if ($Obj -is [Collections.IDictionary]) { if ($Obj.Contains($Name)) { return $Obj[$Name] } else { return $null } }
  $p = $Obj.PSObject.Properties[$Name]
  if ($p) { return $p.Value }
  return $null
}

function Read-JsonFile([string]$Path) {
  $text = Read-SharedText $Path
  if (-not $text) { return $null }
  return ($text | ConvertFrom-Json)
}

# ---------------------------------------------------------------- ci-standard-users 包装

function Invoke-Users([hashtable]$Params) {
  return (& $UsersScript @Params -StateDir $StateDir)
}

function Get-JobDir([string]$Label) { return (Join-Path $StateDir "jobs\$Label") }

function Read-JobRecord([string]$Label) {
  $j = Read-JsonFile (Join-Path (Get-JobDir $Label) 'job.json')
  if ($null -eq $j) { return $null }
  return @($j)[0]
}

function Save-JobOutput([string]$Label) {
  # 只复制输出文件；spec.json、可执行文件不复制
  $dest = Join-Path $ResultsDir $Label
  New-Item -ItemType Directory -Force -Path $dest | Out-Null
  foreach ($f in @('identity.txt', 'stdout.txt', 'stderr.txt', 'exitcode.txt', 'wrapper-error.txt', 'job.json')) {
    $text = Read-SharedText (Join-Path (Get-JobDir $Label) $f)
    if ($null -ne $text) { Set-Content -LiteralPath (Join-Path $dest $f) -Value $text -Encoding UTF8 -NoNewline }
  }
}

function Stop-Job([string]$Label) {
  if (-not $script:Started.Contains($Label)) { return }
  try { Invoke-Users @{ Action = 'Stop'; Label = $Label } | Out-Null } catch { Add-Error "结束 $Label 失败：$($_.Exception.Message)" }
  [void]$script:Started.Remove($Label)
}

function Wait-Path([string]$Path, [int]$Seconds) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  while ((Get-Date) -lt $deadline) {
    if (Test-Path -LiteralPath $Path) { return $true }
    Start-Sleep -Milliseconds 250
  }
  return $false
}

function Get-JobMethod([string]$Label) {
  $job = Read-JobRecord $Label
  if ($job) { return [string]$job.Method }
  return $null
}

# 以 ca_a 身份运行客户端测试，返回 exitCode / method / 解析出的 CA_XUSER_RESULT 行
function Invoke-Client([string]$Label, [string]$Method, [string]$Test, [hashtable]$Environment) {
  $rec = [ordered]@{ label = $Label; method = $null; exitCode = $null; results = @(); error = $null }
  try {
    Invoke-Users @{
      Action = 'Run'; As = 'ca_a'; Label = $Label; Method = $Method; TimeoutSec = $ClientTimeoutSec
      StagePath = @($script:ClientStaged); FilePath = $ClientName; Environment = $Environment
      ArgumentList = @('--ignored', '--exact', $Test, '--nocapture', '--test-threads=1')
    } | Out-Null
    $rec.exitCode = $LASTEXITCODE
  } catch {
    $rec.error = $_.Exception.Message
    Add-Error "$Label 运行失败：$($rec.error)"
  }
  $rec.method = Get-JobMethod $Label
  $out = Read-SharedText (Join-Path (Get-JobDir $Label) 'stdout.txt')
  if ($out) {
    # --nocapture 时 libtest 可能把 `test <名> ... ` 打在同一行前面，所以不锚定行首
    $rec.results = @(($out -split "`r?`n") | ForEach-Object {
        if ($_ -match 'CA_XUSER_RESULT (\{.*\})\s*$') { $Matches[1] | ConvertFrom-Json }
      })
  }
  Save-JobOutput $Label
  return $rec
}

# ---------------------------------------------------------------- 准备

function Install-Mihomo {
  $dl = Join-Path $WorkDir 'download'
  New-Item -ItemType Directory -Force -Path $dl, (Split-Path -Parent $MihomoExe) | Out-Null
  $zip = Join-Path $dl $MihomoAsset
  Write-Info "下载 $MihomoUrl"
  Invoke-WebRequest -Uri $MihomoUrl -OutFile $zip -UseBasicParsing
  $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
  if ($actual -ne $MihomoSha256) { throw "mihomo sha256 不一致：期望 $MihomoSha256，实际 $actual（拒绝继续）" }
  $ex = Join-Path $dl 'extract'
  Expand-Archive -LiteralPath $zip -DestinationPath $ex -Force
  $exes = @(Get-ChildItem -LiteralPath $ex -Recurse -File -Filter '*.exe')
  if ($exes.Count -ne 1) { throw "mihomo zip 中应恰有一个 exe，实际 $($exes.Count) 个" }
  # L2 ① 按映像名比对白名单：改名为 verge-mihomo.exe
  Copy-Item -LiteralPath $exes[0].FullName -Destination $MihomoExe -Force
  return [ordered]@{ version = $MihomoVersion; url = $MihomoUrl; sha256 = $actual }
}

function New-StandardUsers {
  Invoke-Users @{ Action = 'Create'; UserName = @('ca_a', 'ca_b') } | Out-Null
  foreach ($name in @('ca_a', 'ca_b')) {
    $acct = New-Object Security.Principal.NTAccount($env:COMPUTERNAME, $name)
    $sid = $acct.Translate([Security.Principal.SecurityIdentifier]).Value
    # 与 2.1 verge_owner_key 相同：hex(sha256(SID 字符串的 UTF-8 字节))
    $script:Users[$name] = [pscustomobject]@{ Name = $name; Sid = $sid; OwnerKey = (Get-Sha256Hex ([Text.Encoding]::UTF8.GetBytes($sid))) }
  }
}

# ---------------------------------------------------------------- S-D′

# 以目标身份运行的管道服务端（Windows PowerShell 5.1，保持纯 ASCII）。
#   first ：CreateNamedPipeW + 显式 SDDL（首个实例，决定管道级 SD）
#   second：NamedPipeServerStream(MaxAllowedServerInstances)，无 SDDL、无 FirstPipeInstance
# 每次接受连接都记录时间与收到的字节数（读到 EOF 或 2 秒为止），不写任何响应；
# accepted / accepts 与 bytesRead 分开记录（Ellie M-c）。stop 文件出现后退出。
$PipeServerSource = @'
param(
  [Parameter(Mandatory=$true)][string]$Leaf,
  [Parameter(Mandatory=$true)][ValidateSet("first","second")][string]$Role,
  [Parameter(Mandatory=$true)][string]$CoordDir,
  [string]$Sddl
)
$ErrorActionPreference = "Stop"
$here = if ($PSScriptRoot) { $PSScriptRoot } else { (Get-Location).Path }
# standard users cannot write the runner TEMP; csc temp files and the dll live in the job dir
$env:TEMP = $here; $env:TMP = $here
$out = Join-Path $CoordDir ($Role + ".json")
$ready = Join-Path $CoordDir ($Role + ".ready")
$stop = Join-Path $CoordDir "stop"
$doc = [ordered]@{
  schema = "m1-xuser-pipe-server/v1"; role = $Role; leaf = $Leaf; sddl = $Sddl; pid = $PID
  user = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
  createOk = $false; win32 = $null; message = $null
  accepted = $false; accepts = 0; bytesRead = 0; connections = @(); stoppedAt = $null
}
function Save-Doc { ($doc | ConvertTo-Json -Depth 6) | Set-Content -LiteralPath $out -Encoding UTF8 }
try {
  if ($Role -eq "first") {
    $dll = Join-Path $here "CaXuserPipe.dll"
    if (-not (Test-Path -LiteralPath $dll)) {
      $src = 'using System; using System.Runtime.InteropServices; using Microsoft.Win32.SafeHandles;
public static class CaXuserPipe {
  [StructLayout(LayoutKind.Sequential)] public struct SA { public int nLength; public IntPtr sd; public int inherit; }
  [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(string s, uint rev, out IntPtr sd, IntPtr len);
  [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  static extern SafePipeHandle CreateNamedPipeW(string name, uint openMode, uint pipeMode, uint maxInst, uint outBuf, uint inBuf, uint timeout, ref SA sa);
  [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
  public static SafePipeHandle Create(string name, string sddl, out int err) {
    IntPtr sd; err = 0;
    if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, 1, out sd, IntPtr.Zero)) { err = Marshal.GetLastWin32Error(); return null; }
    try {
      SA sa = new SA(); sa.nLength = Marshal.SizeOf(typeof(SA)); sa.sd = sd;
      // PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED; byte mode, PIPE_REJECT_REMOTE_CLIENTS (go-winio)
      SafePipeHandle h = CreateNamedPipeW(@"\\.\pipe\" + name, 0x3u | 0x80000u | 0x40000000u, 0x8u, 255u, 4096u, 4096u, 0u, ref sa);
      if (h.IsInvalid) { err = Marshal.GetLastWin32Error(); return null; }
      return h;
    } finally { LocalFree(sd); }
  }
}'
      Add-Type -OutputAssembly $dll -TypeDefinition $src
    }
    Add-Type -Path $dll
    $err = 0
    $h = [CaXuserPipe]::Create($Leaf, $Sddl, [ref]$err)
    if ($null -eq $h) { $doc.win32 = $err; throw ("CreateNamedPipeW failed: win32 " + $err) }
    $server = New-Object System.IO.Pipes.NamedPipeServerStream([System.IO.Pipes.PipeDirection]::InOut, $true, $false, $h)
  } else {
    $server = New-Object System.IO.Pipes.NamedPipeServerStream($Leaf, [System.IO.Pipes.PipeDirection]::InOut,
      [System.IO.Pipes.NamedPipeServerStream]::MaxAllowedServerInstances, [System.IO.Pipes.PipeTransmissionMode]::Byte,
      [System.IO.Pipes.PipeOptions]::Asynchronous, 0, 0)
  }
  $doc.createOk = $true
  Save-Doc
  "ready" | Set-Content -LiteralPath $ready -Encoding ASCII
  $buf = New-Object byte[] 4096
  while (-not (Test-Path -LiteralPath $stop)) {
    $ar = $server.BeginWaitForConnection($null, $null)
    while (-not $ar.IsCompleted -and -not (Test-Path -LiteralPath $stop)) { Start-Sleep -Milliseconds 50 }
    if (-not $ar.IsCompleted) { break }
    $server.EndWaitForConnection($ar)
    $conn = [ordered]@{ acceptedAt = (Get-Date).ToString("o"); bytes = 0; end = $null; endedAt = $null }
    $doc.accepted = $true
    $doc.accepts++
    $deadline = (Get-Date).AddSeconds(2)
    while ($true) {
      $rar = $server.BeginRead($buf, 0, $buf.Length, $null, $null)
      while (-not $rar.IsCompleted -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 20 }
      if (-not $rar.IsCompleted) { $conn.end = "read-timeout"; break }
      try { $n = $server.EndRead($rar) } catch { $n = 0; $conn.end = "read-error: " + $_.Exception.Message }
      if ($n -le 0) { if (-not $conn.end) { $conn.end = "eof" }; break }
      $conn.bytes += $n
    }
    $conn.endedAt = (Get-Date).ToString("o")
    $doc.bytesRead += $conn.bytes
    $doc.connections += $conn
    Save-Doc
    try { $server.Disconnect() } catch { }
    if ($conn.end -eq "read-timeout") {
      # a pending read cannot be reused safely; stop serving (the orchestrator treats bytes > 0 / timeout as failure)
      break
    }
  }
  $doc.stoppedAt = (Get-Date).ToString("o")
  Save-Doc
  $server.Dispose()
} catch {
  $doc.message = $_.Exception.Message
  if ($_.Exception.InnerException) { $doc.message = $doc.message + " | " + $_.Exception.InnerException.Message }
  if ($null -eq $doc.win32) {
    $hr = $_.Exception.HResult
    if ($_.Exception.InnerException) { $hr = $_.Exception.InnerException.HResult }
    if (($hr -band 0xFFFF0000) -eq [int]0x80070000) { $doc.win32 = ($hr -band 0xFFFF) }
  }
  Save-Doc
  if (-not (Test-Path -LiteralPath $ready)) { "failed" | Set-Content -LiteralPath $ready -Encoding ASCII }
  exit 1
}
exit 0
'@

function Start-PipeServer([string]$Label, [string]$As, [string]$Role, [string]$Leaf, [string]$Coord, [string]$Sddl) {
  $argList = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', 'pipe-server.ps1',
    '-Leaf', $Leaf, '-Role', $Role, '-CoordDir', $Coord)
  if ($Sddl) { $argList += @('-Sddl', $Sddl) }
  $script:Started.Add($Label)
  Invoke-Users @{
    Action = 'Start'; As = $As; Label = $Label; Method = 'ScheduledTask'
    FilePath = $PwshWin; StagePath = @($script:PipeServerScript); ArgumentList = $argList
  } | Out-Null
  $readyOk = Wait-Path (Join-Path $Coord "$Role.ready") $ReadyWaitSec
  $doc = Read-JsonFile (Join-Path $Coord "$Role.json")
  return [ordered]@{ label = $Label; as = $As; method = (Get-JobMethod $Label); ready = $readyOk; server = $doc }
}

function Invoke-SDPrime {
  $a = $script:Users['ca_a']; $b = $script:Users['ca_b']
  $leaf = "verge-mihomo-sidecar-release-$($a.OwnerKey)"
  $pipe = "$PipePrefix$leaf"
  $coord = Join-Path $StageDir 'sdp-coord'
  New-Item -ItemType Directory -Force -Path $coord | Out-Null
  $acl = Get-Acl -LiteralPath $coord
  foreach ($sid in @($a.Sid, $b.Sid)) {
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
        ([Security.Principal.SecurityIdentifier]$sid), 'Modify', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
  }
  Set-Acl -LiteralPath $coord -AclObject $acl
  $script:PipeServerScript = Join-Path $StageDir 'pipe-server.ps1'
  Set-Content -LiteralPath $script:PipeServerScript -Value $PipeServerSource -Encoding ASCII

  $ev = [ordered]@{ pipe = $pipe; sddl = $MihomoDefaultSddl; first = $null; second = $null; client = $null }
  try {
    $ev.first = Start-PipeServer 'sdp-first' 'ca_a' 'first' $leaf $coord $MihomoDefaultSddl
    $firstOk = $ev.first.ready -and $ev.first.server -and $ev.first.server.createOk
    Add-Assert 'sdp: ca_a 以 mihomo 默认 SDDL 创建首个实例' ([bool]$firstOk) 'createOk, user = ca_a' `
      ("createOk=$(if ($ev.first.server) { $ev.first.server.createOk }) user=$(if ($ev.first.server) { $ev.first.server.user }) win32=$(if ($ev.first.server) { $ev.first.server.win32 })")
    if (-not $firstOk) { return $ev }

    $ev.second = Start-PipeServer 'sdp-second' 'ca_b' 'second' $leaf $coord $null
    $secondOk = $ev.second.ready -and $ev.second.server -and $ev.second.server.createOk -and ($ev.second.server.user -eq $b.Sid)
    Add-Assert 'sdp: ca_b 挂上第二实例（BU 含 0x4）' ([bool]$secondOk) 'createOk, user = ca_b' `
      ("createOk=$(if ($ev.second.server) { $ev.second.server.createOk }) user=$(if ($ev.second.server) { $ev.second.server.user }) win32=$(if ($ev.second.server) { $ev.second.server.win32 }) message=$(if ($ev.second.server) { $ev.second.server.message })")
    if (-not $secondOk) { return $ev }

    $ev.client = Invoke-Client 'sdp-client' 'ScheduledTask' $ClientTestPipe @{ CA_XUSER_PIPE = $pipe; CA_XUSER_CONNECTS = [string]$PipeConnects }
  } finally {
    'stop' | Set-Content -LiteralPath (Join-Path $coord 'stop') -Encoding ASCII
    foreach ($l in @('sdp-first', 'sdp-second')) {
      if ($script:Started.Contains($l)) {
        if (-not (Wait-Path (Join-Path (Get-JobDir $l) 'exitcode.txt') 15)) { Stop-Job $l } else { [void]$script:Started.Remove($l) }
        Save-JobOutput $l
      }
    }
    foreach ($r in @('first', 'second')) {
      $f = Join-Path $coord "$r.json"
      if (Test-Path -LiteralPath $f) { Copy-Item -LiteralPath $f -Destination (Join-Path $ResultsDir "sdp-$r-server.json") -Force }
      $key = $r
      if ($ev[$key]) { $ev[$key].server = Read-JsonFile $f }
    }
  }

  $c = $ev.client
  $rows = @($c.results)
  $bad = @($rows | Where-Object { (Get-Prop $_ 'outcome') -ne 'InsecureDacl' -or (Get-Prop $_ 'reason') -ne 'dacl-allows-foreign-instance' })
  Add-Assert 'sdp: 客户端（ca_a, ScheduledTask）每次都是 InsecureDacl / dacl-allows-foreign-instance' `
    ([bool]($c.exitCode -eq 0 -and $rows.Count -eq $PipeConnects -and $bad.Count -eq 0 -and $c.method -eq 'ScheduledTask')) `
    "exit 0, $PipeConnects 次全部 InsecureDacl" `
    ("exit=$($c.exitCode) method=$($c.method) results=$($rows.Count) bad=$($bad.Count) outcomes=$((@($rows | ForEach-Object { "$(Get-Prop $_ 'outcome')/$(Get-Prop $_ 'reason')" })) -join ',')")
  $owners = @($rows | ForEach-Object { Get-Prop $_ 'ownerSid' } | Select-Object -Unique)
  Add-Assert 'sdp: 客户端读到的属主为 ca_a（管道级 SD）' ([bool]($owners.Count -eq 1 -and $owners[0] -eq $a.Sid)) $a.Sid ($owners -join ',')

  $s1 = $ev.first.server; $s2 = $ev.second.server
  # Ellie M-c：先证明外人实例确实接受过连接，0 字节断言才有意义
  Add-Assert 'sdp: ca_b 的第二实例确实接受过连接（accepted）' ([bool]($s2 -and $s2.accepted -and $s2.accepts -ge 1)) 'accepted=true, accepts >= 1' `
    ("accepted=$(if ($s2) { $s2.accepted }) accepts=$(if ($s2) { $s2.accepts })")
  $timeouts = @(@($s1, $s2) | Where-Object { $_ } | ForEach-Object { @($_.connections) } | Where-Object { $_.end -ne 'eof' })
  Add-Assert 'sdp: 每个被接受的连接都以 EOF 结束（客户端拒绝后直接关闭）' ([bool]($timeouts.Count -eq 0)) 'end = eof' `
    ((@($timeouts | ForEach-Object { $_.end })) -join ',')
  Add-Assert 'sdp: 两个实例收到的字节数都为 0（F5）' ([bool]($s1 -and $s2 -and $s1.bytesRead -eq 0 -and $s2.bytesRead -eq 0)) 'first=0, second=0' `
    ("first=$(if ($s1) { $s1.bytesRead }) second=$(if ($s2) { $s2.bytesRead })")
  # 每次连接都落在这两个实例之一（没有别的实例），首个实例的 accepted 只作记录
  $sum = if ($s1 -and $s2) { $s1.accepts + $s2.accepts } else { $null }
  Add-Assert 'sdp: 两个实例接受的连接数之和等于客户端连接次数' ([bool]($sum -eq $PipeConnects)) $PipeConnects `
    ("$sum（first accepted=$(if ($s1) { $s1.accepted }) accepts=$(if ($s1) { $s1.accepts })）")
  return $ev
}

# ---------------------------------------------------------------- 规则 4：token-mismatch / unverified

function Invoke-TcpCase([string]$Method, [int]$Port, [string]$Expect) {
  $b = $script:Users['ca_b']
  $tag = $Method.ToLowerInvariant()
  $mLabel = "tcp-$tag-mihomo"
  $conf = Join-Path $StageDir "$mLabel\conf"
  New-Item -ItemType Directory -Force -Path $conf | Out-Null
  Set-Content -LiteralPath (Join-Path $conf 'config.yaml') -Encoding UTF8 -Value @('mode: direct', 'log-level: info', "external-controller: ''")
  $ext = "127.0.0.1:$Port"
  $ev = [ordered]@{ method = $Method; expect = $Expect; listen = $ext; mihomo = $null; client = $null }
  try {
    $script:Started.Add($mLabel)
    Invoke-Users @{
      Action = 'Start'; As = 'ca_b'; Label = $mLabel; Method = $Method
      StagePath = @($MihomoExe, $conf); FilePath = 'verge-mihomo.exe'
      ArgumentList = @('-d', (Join-Path (Get-JobDir $mLabel) 'conf'), '-ext-ctl', $ext)
    } | Out-Null
    $job = Read-JobRecord $mLabel
    $mpid = if ($job) { [int]$job.Pid } else { 0 }
    $status = 'timeout'
    $deadline = (Get-Date).AddSeconds($ReadyWaitSec)
    while ((Get-Date) -lt $deadline) {
      $log = Read-SharedText (Join-Path (Get-JobDir $mLabel) 'stdout.txt')
      if ($log -and $log -match 'RESTful API listening at') { $status = 'listening'; break }
      if ($log -and $log -match 'External controller listen error') { $status = 'listen-error'; break }
      if ($mpid -and -not (Get-Process -Id $mpid -ErrorAction SilentlyContinue)) { $status = 'exited'; break }
      Start-Sleep -Milliseconds 300
    }
    $ev.mihomo = [ordered]@{ pid = $mpid; method = (Get-JobMethod $mLabel); status = $status }
    Add-Assert "tcp-${tag}: ca_b 的 verge-mihomo.exe（$Method）在 $ext 监听" ([bool]($status -eq 'listening' -and $ev.mihomo.method -eq $Method)) "listening, method=$Method" "status=$status method=$($ev.mihomo.method)"
    if ($status -ne 'listening') { return $ev }

    $ev.client = Invoke-Client "tcp-$tag-client" $Method $ClientTestTcp @{ CA_XUSER_TCP_ADDR = $ext; CA_XUSER_TCP_EXPECT = $Expect }
    $c = $ev.client
    $r = if (@($c.results).Count -eq 1) { @($c.results)[0] } else { $null }
    $ok = $c.exitCode -eq 0 -and $c.method -eq $Method -and $r -and (Get-Prop $r 'code') -eq $Expect -and (Get-Prop $r 'pid') -eq $mpid
    if ($ok -and $Expect -eq 'token-mismatch') { $ok = (Get-Prop $r 'sid') -eq $b.Sid }
    Add-Assert "tcp-${tag}: ca_a（$Method）对 ca_b 的监听者判为 $Expect" ([bool]$ok) "$Expect, pid=$mpid$(if ($Expect -eq 'token-mismatch') { ", sid=$($b.Sid)" })" `
      ("exit=$($c.exitCode) method=$($c.method) code=$(Get-Prop $r 'code') pid=$(Get-Prop $r 'pid') sid=$(Get-Prop $r 'sid') reasons=$((@(Get-Prop $r 'reasons')) -join ' | ')")
  } finally {
    Stop-Job $mLabel
    Save-JobOutput $mLabel
  }
  return $ev
}

# ---------------------------------------------------------------- 清理

function Invoke-ReplayCleanup {
  foreach ($l in @($script:Started)) { Stop-Job $l }
  # 先按映像名结束残留（避免配置文件被占用导致 Cleanup 非零）；runner 自身的步骤在 pwsh 中运行
  foreach ($name in @('verge-mihomo', [IO.Path]::GetFileNameWithoutExtension($ClientName), 'powershell')) {
    foreach ($p in @(Get-Process -Name $name -ErrorAction SilentlyContinue)) {
      try { Stop-Process -Id $p.Id -Force -ErrorAction Stop } catch { }
    }
  }
  Start-Sleep -Milliseconds 500
  try {
    Invoke-Users @{ Action = 'Cleanup'; UserName = @('ca_a', 'ca_b') } | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Info "ci-standard-users Cleanup 退出码 $LASTEXITCODE（workflow 的兜底步骤会再核对用户 / 计划任务是否已删除）" }
  } catch {
    Add-Error "ci-standard-users Cleanup 失败：$($_.Exception.Message)"
  }
  if (Test-Path -LiteralPath $StageDir) {
    try { Remove-Item -LiteralPath $StageDir -Recurse -Force } catch { Write-Info "删除暂存目录失败：$($_.Exception.Message)" }
  }
}

# ---------------------------------------------------------------- main

New-Item -ItemType Directory -Force -Path $ResultsDir | Out-Null
$resolvedResults = (Resolve-Path -LiteralPath $ResultsDir).Path
if ($resolvedResults.StartsWith([IO.Path]::GetFullPath($StateDir), [StringComparison]::OrdinalIgnoreCase)) {
  throw 'ResultsDir 不得位于 ci-standard-users 的状态目录内（其中有 DPAPI 凭据）'
}
if (-not (Test-Path -LiteralPath $ClientExe)) { throw "找不到客户端测试二进制：$ClientExe" }

$summary = [ordered]@{
  replay     = 'M1 跨用户回放（Ellie MUST-4 / §2-11）'
  startedAt  = (Get-Date).ToString('o')
  runner     = [ordered]@{ os = [Environment]::OSVersion.VersionString; ps = $PSVersionTable.PSVersion.ToString() }
  client     = [ordered]@{ path = $ClientExe; sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $ClientExe).Hash.ToLowerInvariant() }
  mihomo     = $null
  users      = $null
  sdPrime    = $null
  tcp        = @()
  assertions = $null
  errors     = $null
  pass       = $false
}

try {
  New-Item -ItemType Directory -Force -Path $StageDir | Out-Null
  $script:ClientStaged = Join-Path $StageDir $ClientName
  Copy-Item -LiteralPath $ClientExe -Destination $script:ClientStaged -Force
  $summary.mihomo = Install-Mihomo
  New-StandardUsers
  $summary.users = @($script:Users.Values | ForEach-Object { [ordered]@{ name = $_.Name; sid = $_.Sid; ownerKey = $_.OwnerKey } })
  try { $summary.sdPrime = Invoke-SDPrime } catch { Add-Error "S-D′ 执行异常：$($_.Exception.Message)" }
  foreach ($t in @(@('Credential', $PortCredential, 'token-mismatch'), @('ScheduledTask', $PortTask, 'unverified'))) {
    try { $summary.tcp += Invoke-TcpCase $t[0] $t[1] $t[2] } catch { Add-Error "规则 4（$($t[0])）执行异常：$($_.Exception.Message)" }
  }
} catch {
  Add-Error "准备阶段失败：$($_.Exception.Message)"
} finally {
  Invoke-ReplayCleanup
  $summary.assertions = $script:Asserts.ToArray()
  $summary.errors = $script:Errors.ToArray()
  $failed = @($script:Asserts | Where-Object { -not $_.pass })
  # 期望的断言数：S-D′ 8 条 + 每个 TCP 用例 2 条（共 12）；缺了说明有步骤没跑到
  $summary.pass = ($script:Errors.Count -eq 0 -and $failed.Count -eq 0 -and $script:Asserts.Count -eq 12)
  $summary['finishedAt'] = (Get-Date).ToString('o')
  Write-Json (Join-Path $ResultsDir 'results.json') $summary
}

if (-not $summary.pass) {
  Write-Info "失败：断言 $($script:Asserts.Count) 条（失败 $(@($script:Asserts | Where-Object { -not $_.pass }).Count)），错误 $($script:Errors.Count) 个，见 results.json"
  exit 1
}
Write-Info "通过：$($script:Asserts.Count) 条断言"
exit 0
