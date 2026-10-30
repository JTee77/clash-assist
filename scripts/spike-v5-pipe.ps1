<#
.SYNOPSIS
  V5 pipe spike 编排：windows-plan r4 §4.1，用例 S-A ~ S-F。只存在于 win/v5-pipe-spike 分支，不合并。

.DESCRIPTION
  由 .github/workflows/spike-v5-pipe.yml 在 windows-2022 runner 上调用（shell: pwsh）。依次：
    1. 下载 mihomo v1.19.33（MetaCubeX 官方 release 的 windows-amd64-compatible zip），校验 sha256，
       不一致直接失败；解压后改名为 verge-mihomo.exe（L2 检查映像名）。
    2. 经 scripts/ci-standard-users.ps1 创建标准用户 ca_a、ca_b（口令只在内存 / DPAPI 中）。
    3. 按 §4.1 的表格以不同身份运行 mihomo 与探针 spike/pipe-probe，逐步记录结果。
    4. finally 中结束进程、删除计划任务、删除用户及其配置文件（workflow 另有 if: always() 的清理步骤兜底）。

  本脚本与探针一样**只记录，不判定**：P1 ~ P7 的判定由人工（Ellie / Pit）读结果 JSON 完成。
  每一步写一个 JSON 到 <ResultsDir>\<用例>\<步骤>.json，原始输出（identity / stdout / stderr）放在
  同名目录下；汇总写到 <ResultsDir>\summary.json。ResultsDir 会作为 artifact 上传，
  因此只从作业目录复制这些输出文件，不复制 spec.json（其中有 S-F 的 secret 环境变量）与 secrets\。

  探针的 CLI 约定尚未发布：所有探针调用集中在“探针调用层”的三个函数中，
  标有 TODO(probe-cli)，约定确定后只改这三个函数。探针不存在时，探针步骤记为 probe-missing，
  mihomo 与身份部分照常运行（可先验证 runner 能否以其他用户 / SYSTEM 运行），脚本最终以 1 退出。

.PARAMETER ResultsDir
  结果目录（artifact）。不得位于 ci-standard-users 的状态目录内。

.PARAMETER ProbePath
  pipe-probe.exe 的路径。省略或文件不存在时，探针步骤记为 probe-missing。

.PARAMETER WorkDir
  下载与暂存目录，默认 $env:RUNNER_TEMP\v5-pipe-spike。

.NOTES
  需要管理员权限（runner 默认满足）；只在 PowerShell 7（pwsh）下使用。
  退出码：0 = 全部步骤已执行（不代表 P1 ~ P7 通过）；1 = 有步骤出错、探针缺失或清理失败。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$ResultsDir,

  [string]$ProbePath,

  [string]$WorkDir
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # 否则 Invoke-WebRequest 的进度条显著拖慢下载

# mihomo：§4.1 第 1 步。期望 sha256 取自 GitHub release 资产 digest
# （gh api repos/MetaCubeX/mihomo/releases/tags/v1.19.33 → assets[].digest），与方案正文一致。
$MihomoVersion = 'v1.19.33'
$MihomoAsset = "mihomo-windows-amd64-compatible-$MihomoVersion.zip"
$MihomoUrl = "https://github.com/MetaCubeX/mihomo/releases/download/$MihomoVersion/$MihomoAsset"
$MihomoSha256 = '7aaf636a4f2172dcd8ec34ac4f52797b059195e5afd6409508c963e545f8864d'

$SidSystem = 'S-1-5-18'
$SidAdmins = 'S-1-5-32-544'
$PipePrefix = '\\.\pipe\'
$PortServiceCtl = 19098   # S-C：SYSTEM 核心的外部控制器
$PortForeignCtl = 19097   # S-F：ca_b 核心的外部控制器
$ListenWaitSec = 30
$ProbeTimeoutSec = 60
$PipeGoneWaitSec = 10

$base = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
if (-not $WorkDir) { $WorkDir = Join-Path $base 'v5-pipe-spike' }
# 与 ci-standard-users.ps1 的默认值相同，显式传入以便定位作业目录
$StateDir = Join-Path $base 'ca-standard-users'
$UsersScript = Join-Path $PSScriptRoot 'ci-standard-users.ps1'
$MihomoExe = Join-Path $WorkDir 'bin\verge-mihomo.exe'
$StageDir = Join-Path $WorkDir 'stage'

$script:Steps = New-Object System.Collections.Generic.List[object]
$script:Errors = New-Object System.Collections.Generic.List[string]
$script:MethodFor = @{}          # 身份 -> 实际可用的启动方式（首次 Auto 探测后固定）
$script:RunningMihomo = New-Object System.Collections.Generic.List[string]
$script:Scrub = New-Object System.Collections.Generic.List[string]   # 复制输出前替换掉的机密
$script:Users = @{}

function Write-Info([string]$Message) { Write-Host "[spike-v5] $Message" }

function Add-SpikeError([string]$Message) {
  $script:Errors.Add($Message)
  Write-Warning $Message
}

function Write-Json([string]$Path, $Value) {
  $dir = Split-Path -Parent $Path
  if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
  ConvertTo-Json -InputObject $Value -Depth 10 | Set-Content -LiteralPath $Path -Encoding UTF8
}

function Get-Sha256Hex([byte[]]$Bytes) {
  $sha = [Security.Cryptography.SHA256]::Create()
  try { $hash = $sha.ComputeHash($Bytes) } finally { $sha.Dispose() }
  return (($hash | ForEach-Object { $_.ToString('x2') }) -join '')
}

function New-RandomHex([int]$ByteCount) {
  $b = New-Object byte[] $ByteCount
  $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $rng.GetBytes($b) } finally { $rng.Dispose() }
  return (($b | ForEach-Object { $_.ToString('x2') }) -join '')
}

function Read-SharedText([string]$Path) {
  # 目标进程可能仍在写（stdout 重定向），按共享读打开
  if (-not (Test-Path -LiteralPath $Path)) { return $null }
  $fs = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
  try {
    $sr = New-Object IO.StreamReader($fs)
    try { return $sr.ReadToEnd() } finally { $sr.Dispose() }
  } finally { $fs.Dispose() }
}

function Hide-Secret([string]$Text) {
  if ($null -eq $Text) { return $null }
  foreach ($s in $script:Scrub) { $Text = $Text.Replace($s, '***') }
  return $Text
}

# ---------------------------------------------------------------- ci-standard-users 包装

function Invoke-Users([hashtable]$Params) {
  return (& $UsersScript @Params -StateDir $StateDir)
}

function Get-JobDir([string]$Label) { return (Join-Path $StateDir "jobs\$Label") }

function Get-MethodParam([string]$As) {
  if ($script:MethodFor.ContainsKey($As)) { return @{ Method = $script:MethodFor[$As] } }
  return @{}
}

function Read-JobRecord([string]$Label) {
  $f = Join-Path (Get-JobDir $Label) 'job.json'
  if (-not (Test-Path -LiteralPath $f)) { return $null }
  return @((Get-Content -Raw -Encoding UTF8 -LiteralPath $f | ConvertFrom-Json) | ForEach-Object { $_ })[0]
}

function Save-JobOutput([string]$Label, [string]$Dest) {
  # 只复制输出文件；spec.json（含环境变量）、conf\、可执行文件一律不复制
  $jobDir = Get-JobDir $Label
  if (-not (Test-Path -LiteralPath $Dest)) { New-Item -ItemType Directory -Path $Dest | Out-Null }
  foreach ($f in @('identity.txt', 'stdout.txt', 'stderr.txt', 'exitcode.txt', 'wrapper-error.txt', 'job.json')) {
    $text = Read-SharedText (Join-Path $jobDir $f)
    if ($null -ne $text) { Set-Content -LiteralPath (Join-Path $Dest $f) -Value (Hide-Secret $text) -Encoding UTF8 -NoNewline }
  }
}

function Get-IdentityInfo([string]$Label) {
  # P7 环境代表性：运行身份的令牌中不应出现 Administrators（S-1-5-32-544）
  $text = Read-SharedText (Join-Path (Get-JobDir $Label) 'identity.txt')
  if ($null -eq $text) { return [ordered]@{ captured = $false; adminsGroupPresent = $null } }
  return [ordered]@{ captured = $true; adminsGroupPresent = ($text -match [regex]::Escape($SidAdmins)) }
}

function Remember-Method([string]$As, [string]$Label) {
  $job = Read-JobRecord $Label
  if ($job -and $job.Method -and -not $script:MethodFor.ContainsKey($As)) {
    $script:MethodFor[$As] = [string]$job.Method
    Write-Info "$As 的启动方式确定为 $($job.Method)"
  }
  if ($job) { return [string]$job.Method }
  return $null
}

# ---------------------------------------------------------------- 管道辅助（只枚举名字，不打开管道）

function Test-PipePresent([string]$PipeName) {
  try {
    return (@([IO.Directory]::GetFiles($PipePrefix)) -contains $PipeName)
  } catch {
    return $null
  }
}

function Wait-PipeGone([string]$PipeName) {
  $deadline = (Get-Date).AddSeconds($PipeGoneWaitSec)
  while ((Get-Date) -lt $deadline) {
    if ((Test-PipePresent $PipeName) -ne $true) { return $true }
    Start-Sleep -Milliseconds 250
  }
  return $false
}

# ---------------------------------------------------------------- 准备

function Install-Mihomo {
  $dl = Join-Path $WorkDir 'download'
  New-Item -ItemType Directory -Force -Path $dl, (Split-Path -Parent $MihomoExe) | Out-Null
  $zip = Join-Path $dl $MihomoAsset
  Write-Info "下载 $MihomoUrl"
  Invoke-WebRequest -Uri $MihomoUrl -OutFile $zip -UseBasicParsing
  $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
  if ($actual -ne $MihomoSha256) {
    throw "mihomo sha256 不一致：期望 $MihomoSha256，实际 $actual（拒绝继续）"
  }
  $ex = Join-Path $dl 'extract'
  Expand-Archive -LiteralPath $zip -DestinationPath $ex -Force
  $exes = @(Get-ChildItem -LiteralPath $ex -Recurse -File -Filter '*.exe')
  if ($exes.Count -ne 1) { throw "mihomo zip 中应恰有一个 exe，实际 $($exes.Count) 个" }
  Copy-Item -LiteralPath $exes[0].FullName -Destination $MihomoExe -Force
  # 版本输出只作记录，失败不阻断（sha256 已是准入条件）
  try {
    $ErrorActionPreference = 'Continue'
    $ver = (& $MihomoExe -v 2>&1 | Out-String).Trim()
  } catch {
    $ver = "无法执行 -v：$($_.Exception.Message)"
  }
  return [ordered]@{
    version     = $MihomoVersion
    asset       = $MihomoAsset
    url         = $MihomoUrl
    sha256      = $actual
    verified    = $true
    innerExe    = $exes[0].Name
    renamedTo   = 'verge-mihomo.exe'
    versionText = $ver
  }
}

function New-StandardUsers {
  Invoke-Users @{ Action = 'Create'; UserName = @('ca_a', 'ca_b') } | Out-Null
  foreach ($name in @('ca_a', 'ca_b')) {
    $acct = New-Object Security.Principal.NTAccount($env:COMPUTERNAME, $name)
    $sid = $acct.Translate([Security.Principal.SecurityIdentifier]).Value
    # 与 ci-standard-users.ps1 / 2.1 verge_owner_key 相同：hex(sha256(SID 字符串的 UTF-8 字节))
    $script:Users[$name] = [pscustomobject]@{
      Name     = $name
      Sid      = $sid
      OwnerKey = Get-Sha256Hex ([Text.Encoding]::UTF8.GetBytes($sid))
    }
  }
}

function Get-VergeSddl([string]$UserSid) {
  return "D:P(A;;GA;;;$UserSid)(A;;GA;;;SY)(A;;GA;;;BA)"
}

# ---------------------------------------------------------------- mihomo

function Start-Mihomo {
  param(
    [string]$CaseId, [string]$Label, [string]$As,
    [string]$PipeName, [string]$Sddl, [string]$ExtCtl, [string]$Secret
  )
  # §4.1 第 2 步：最小配置，无规则、无节点
  $conf = Join-Path $StageDir "$Label\conf"
  New-Item -ItemType Directory -Force -Path $conf | Out-Null
  Set-Content -LiteralPath (Join-Path $conf 'config.yaml') -Encoding UTF8 -Value @(
    'mode: direct'
    'log-level: debug'
    "external-controller: ''"
  )
  $jobDir = Get-JobDir $Label
  $argList = @('-d', (Join-Path $jobDir 'conf'))
  if ($PipeName) { $argList += @('-ext-ctl-pipe', $PipeName) }
  if ($ExtCtl) { $argList += @('-ext-ctl', $ExtCtl) }
  $envMap = @{}
  if ($Sddl) { $envMap['LISTEN_NAMEDPIPE_SDDL'] = $Sddl }
  # secret 经环境变量（mihomo 的 CLASH_OVERRIDE_SECRET）传入：不进命令行，也不写进结果目录
  if ($Secret) { $envMap['CLASH_OVERRIDE_SECRET'] = $Secret }

  $rec = [ordered]@{
    case = $CaseId; step = $Label; kind = 'mihomo'; as = $As
    pipe = $PipeName; sddl = $Sddl; externalController = $ExtCtl; secretSet = [bool]$Secret
    method = $null; pid = $null; pipeStatus = $null; tcpStatus = $null; logLines = @(); error = $null
  }
  if ($PipeName -and (Test-PipePresent $PipeName) -eq $true) {
    $rec.error = "启动前管道已存在：$PipeName"
    return $rec
  }
  $params = @{
    Action = 'Start'; As = $As; Label = $Label
    StagePath = @($MihomoExe, $conf); FilePath = 'verge-mihomo.exe'
    ArgumentList = $argList; Environment = $envMap
  } + (Get-MethodParam $As)
  $script:RunningMihomo.Add($Label)
  Invoke-Users $params | Out-Null
  $rec.method = Remember-Method $As $Label
  $job = Read-JobRecord $Label
  if ($job) { $rec.pid = [int]$job.Pid }

  # 等待监听结果：管道监听失败时 mihomo 只记错误日志、不退出（hub/route/server.go startPipe），
  # 所以以日志行为准，S-E ①②③ 的 Win32 错误文本也从这里取得
  $deadline = (Get-Date).AddSeconds($ListenWaitSec)
  $jobDirOut = Join-Path $jobDir 'stdout.txt'
  while ((Get-Date) -lt $deadline) {
    $log = Read-SharedText $jobDirOut
    if ($null -eq $log) { $log = '' }
    if ($PipeName -and -not $rec.pipeStatus) {
      if ($log -match 'RESTful API pipe listening at') { $rec.pipeStatus = 'listening' }
      elseif ($log -match 'External controller pipe listen error') { $rec.pipeStatus = 'listen-error' }
    }
    if ($ExtCtl -and -not $rec.tcpStatus) {
      if ($log -match 'RESTful API listening at') { $rec.tcpStatus = 'listening' }
      elseif ($log -match 'External controller listen error') { $rec.tcpStatus = 'listen-error' }
    }
    $pipeDone = (-not $PipeName) -or $rec.pipeStatus
    $tcpDone = (-not $ExtCtl) -or $rec.tcpStatus
    if ($pipeDone -and $tcpDone) { break }
    if ($rec.pid -and -not (Get-Process -Id $rec.pid -ErrorAction SilentlyContinue)) {
      if ($PipeName -and -not $rec.pipeStatus) { $rec.pipeStatus = 'exited' }
      if ($ExtCtl -and -not $rec.tcpStatus) { $rec.tcpStatus = 'exited' }
      break
    }
    Start-Sleep -Milliseconds 500
  }
  if ($PipeName -and -not $rec.pipeStatus) { $rec.pipeStatus = 'timeout' }
  if ($ExtCtl -and -not $rec.tcpStatus) { $rec.tcpStatus = 'timeout' }
  if ($PipeName) { $rec['pipePresent'] = Test-PipePresent $PipeName }
  $log = Read-SharedText $jobDirOut
  if ($log) {
    $rec.logLines = @(($log -split "`r?`n") | Where-Object { $_ -match 'RESTful API|External controller|error|fatal' } |
        ForEach-Object { Hide-Secret $_ })
  }
  $rec['identity'] = Get-IdentityInfo $Label
  Write-Info "$Label（$As）：pipe=$($rec.pipeStatus) tcp=$($rec.tcpStatus) method=$($rec.method)"
  return $rec
}

function Stop-Mihomo([string]$CaseId, [string]$Label, [string]$PipeName) {
  try {
    Invoke-Users @{ Action = 'Stop'; Label = $Label } | Out-Null
  } catch {
    Add-SpikeError "$CaseId 结束 $Label 失败：$($_.Exception.Message)"
  }
  [void]$script:RunningMihomo.Remove($Label)
  Save-JobOutput $Label (Join-Path $ResultsDir "$CaseId\$Label")
  if ($PipeName -and -not (Wait-PipeGone $PipeName)) {
    Add-SpikeError "$CaseId 结束 $Label 后 $PipeName 仍存在"
  }
}

function Add-Step([string]$CaseId, [string]$Step, $Record) {
  Write-Json (Join-Path $ResultsDir "$CaseId\$Step.json") $Record
  $status = if ($Record.Contains('status')) { $Record.status } elseif ($Record.error) { 'error' } else { 'ran' }
  $script:Steps.Add([ordered]@{ case = $CaseId; step = $Step; status = $status; file = "$CaseId/$Step.json" })
}

# ---------------------------------------------------------------- 探针（通用执行）

function Invoke-Probe {
  param([string]$CaseId, [string]$Step, [string]$As, [string[]]$ProbeArgs, [string]$Expect)
  $rec = [ordered]@{
    case = $CaseId; step = $Step; kind = 'probe'; as = $As; args = $ProbeArgs; expect = $Expect
    status = $null; method = $null; exitCode = $null; timedOut = $false
    output = $null; outputFormat = $null; identity = $null; error = $null
    verdict = $null   # 只记录；P1 ~ P7 由人工判定
  }
  if (-not $ProbePath -or -not (Test-Path -LiteralPath $ProbePath)) {
    $rec.status = 'probe-missing'
    Add-Step $CaseId $Step $rec
    return $rec
  }
  $params = @{
    Action = 'Run'; As = $As; Label = $Step
    StagePath = @($ProbePath); FilePath = (Split-Path -Leaf $ProbePath)
    ArgumentList = $ProbeArgs; TimeoutSec = $ProbeTimeoutSec
  } + (Get-MethodParam $As)
  try {
    Invoke-Users $params | Out-Null
    $rec.exitCode = $LASTEXITCODE
    $rec.timedOut = ($rec.exitCode -eq 124)
    $rec.status = if ($rec.timedOut) { 'timeout' } else { 'ran' }
  } catch {
    $rec.status = 'error'
    $rec.error = $_.Exception.Message
  }
  $rec.method = Remember-Method $As $Step
  $rec.identity = Get-IdentityInfo $Step
  # TODO(probe-cli): 输出格式待定；目前假设 stdout 为一个 JSON 文档，解析失败则保留原文
  $out = Read-SharedText (Join-Path (Get-JobDir $Step) 'stdout.txt')
  if ($out) {
    try {
      $rec.output = $out | ConvertFrom-Json
      $rec.outputFormat = 'json'
    } catch {
      $rec.output = Hide-Secret $out
      $rec.outputFormat = 'raw'
    }
  }
  Save-JobOutput $Step (Join-Path $ResultsDir "$CaseId\$Step")
  Add-Step $CaseId $Step $rec
  if ($rec.status -eq 'error') { Add-SpikeError "$CaseId/$Step：$($rec.error)" }
  return $rec
}

# ---------------------------------------------------------------- 探针调用层
# TODO(probe-cli): fill once Pit publishes the CLI contract
# 下面三个函数是探针 CLI 的唯一接入点；参数形式目前按 §4.1 第 4 步的描述占位。

function Invoke-ProbePipe([string]$CaseId, [string]$Step, [string]$As, [string]$PipeName, [string]$Expect) {
  # TODO(probe-cli): fill once Pit publishes the CLI contract（§4.1：pipe <name>，SECURITY_ANONYMOUS 打开，
  # 记录属主 SID、服务端 PID、L2 ①②③、L3、WTS、GET /version）
  return (Invoke-Probe -CaseId $CaseId -Step $Step -As $As -ProbeArgs @('pipe', $PipeName) -Expect $Expect)
}

function Invoke-ProbeTcp([string]$CaseId, [string]$Step, [string]$As, [int]$Port, [string]$Expect) {
  # TODO(probe-cli): fill once Pit publishes the CLI contract（§4.1：tcp-probe <port>，
  # GetExtendedTcpTable、L2 ①②③、L3、WTS，不发送请求）
  return (Invoke-Probe -CaseId $CaseId -Step $Step -As $As -ProbeArgs @('tcp-probe', [string]$Port) -Expect $Expect)
}

function Invoke-ProbeSquat {
  param([string]$CaseId, [string]$Step, [string]$As, [string]$PipeName, [string]$Sddl, [string]$SetOwnerSid, [string]$Expect)
  # TODO(probe-cli): fill once Pit publishes the CLI contract（§4.1 / S-R9：squat <name> <sddl>；
  # ⑥ 直接以含 O: 的 SDDL 创建；⑤ 正常创建后 SetSecurityInfo(OWNER) 改为 SetOwnerSid——
  # ⑤ 的参数形式未定，这里暂用 `--set-owner <sid>` 占位）
  $probeArgs = @('squat', $PipeName, $Sddl)
  if ($SetOwnerSid) { $probeArgs += @('--set-owner', $SetOwnerSid) }
  return (Invoke-Probe -CaseId $CaseId -Step $Step -As $As -ProbeArgs $probeArgs -Expect $Expect)
}

# ---------------------------------------------------------------- 用例

function Invoke-WithMihomo {
  # 启动 mihomo → 执行 $Body（参数为 mihomo 记录；按动态作用域读取调用方的变量）→ 无论成败都结束 mihomo 并保存其输出
  param([string]$CaseId, [hashtable]$Mihomo, [scriptblock]$Body)
  $label = $Mihomo.Label
  $rec = $null
  try {
    $rec = Start-Mihomo -CaseId $CaseId @Mihomo
    if ($rec.error) {
      Add-SpikeError "$CaseId/$label：$($rec.error)"
    } else {
      & $Body $rec
    }
  } catch {
    Add-SpikeError "$CaseId/$label：$($_.Exception.Message)"
    if ($null -eq $rec) { $rec = [ordered]@{ case = $CaseId; step = $label; kind = 'mihomo' } }
    $rec['error'] = $_.Exception.Message
  } finally {
    if ($script:RunningMihomo.Contains($label)) { Stop-Mihomo $CaseId $label $Mihomo['PipeName'] }
    if ($null -ne $rec) { Add-Step $CaseId $label $rec }
  }
}

function Invoke-CaseAB {
  # S-A（P1/P2/P3）与 S-B（P4）共用同一个 ca_a 核心
  $a = $script:Users['ca_a']
  $pipe = "${PipePrefix}verge-mihomo-sidecar-release-$($a.OwnerKey)"
  Invoke-WithMihomo 'S-A' @{ Label = 'sa-mihomo'; As = 'ca_a'; PipeName = $pipe; Sddl = (Get-VergeSddl $a.Sid) } {
    Invoke-ProbePipe 'S-A' 'sa-probe' 'ca_a' $pipe 'P1 /version 200；P2 属主可读且 = ca_a SID；P3 L2 ① 成功、映像名 verge-mihomo.exe' | Out-Null
    Invoke-ProbePipe 'S-B' 'sb-probe' 'ca_b' $pipe 'P4 打开失败，ERROR_ACCESS_DENIED' | Out-Null
  }
}

function Invoke-CaseC {
  $a = $script:Users['ca_a']
  $pipe = "${PipePrefix}verge-mihomo-production-$($a.OwnerKey)"
  $mihomo = @{ Label = 'sc-mihomo'; As = 'SYSTEM'; PipeName = $pipe; Sddl = (Get-VergeSddl $a.Sid); ExtCtl = "127.0.0.1:$PortServiceCtl" }
  Invoke-WithMihomo 'S-C' $mihomo {
    Invoke-ProbePipe 'S-C' 'sc-probe-pipe' 'ca_a' $pipe 'P5 /version 200，属主为 SYSTEM 或 BA；记录 L2 ①②③（预期 ① ACCESS_DENIED，② 或 ③ 成功）、L3、WTS pUserSid' | Out-Null
    Invoke-ProbeTcp 'S-C' 'sc-probe-tcp' 'ca_a' $PortServiceCtl '按 2.1 规则 4 应为 listener-unverified（C-4）；WTS 只记录' | Out-Null
  }
}

function Invoke-CaseD {
  $a = $script:Users['ca_a']
  $pipe = "${PipePrefix}verge-mihomo-sidecar-release-$($a.OwnerKey)"
  # 不设 LISTEN_NAMEDPIPE_SDDL：mihomo 默认 D:PAI(A;OICI;GWGR;;;BU)(A;OICI;GWGR;;;SY)
  Invoke-WithMihomo 'S-D' @{ Label = 'sd-mihomo'; As = 'ca_a'; PipeName = $pipe } {
    Invoke-ProbePipe 'S-D' 'sd-probe' 'ca_b' $pipe '仅记录：ca_b 能否连上、S-2 的 ACE 遍历结果' | Out-Null
  }
}

function Invoke-CaseE {
  $a = $script:Users['ca_a']
  $pipe = "${PipePrefix}verge-mihomo-sidecar-release-$($a.OwnerKey)"
  $dacl = 'D:(A;;GA;;;WD)'
  $owners = [ordered]@{ ca_a = $a.Sid; sy = 'SY'; ba = 'BA' }

  # ①②③：ca_b 用真实 mihomo 以伪造属主抢注；④：不带 O:（P7 对照）
  $mihomoTries = @(
    @{ n = 'e1'; sddl = "O:$($a.Sid)$dacl"; expect = 'P6 伪造失败：mihomo 记录 listen error，或 ca_a 读到的属主不是 ca_a/SYSTEM/BA' }
    @{ n = 'e2'; sddl = "O:SY$dacl"; expect = 'P6 伪造失败（同 ①，目标 SYSTEM）' }
    @{ n = 'e3'; sddl = "O:BA$dacl"; expect = 'P6 伪造失败（同 ①，目标 BA）' }
    @{ n = 'e4'; sddl = $dacl; expect = 'P7 ca_a 能连上，读到的属主 = ca_b SID' }
  )
  foreach ($t in $mihomoTries) {
    $label = "se-$($t.n)-mihomo"
    $expect = $t.expect
    Invoke-WithMihomo 'S-E' @{ Label = $label; As = 'ca_b'; PipeName = $pipe; Sddl = $t.sddl } {
      param($m)
      if ($m.pipeStatus -eq 'listening') {
        Invoke-ProbePipe 'S-E' "se-$($t.n)-probe" 'ca_a' $pipe $expect | Out-Null
      } else {
        Write-Info "S-E $($t.n)：mihomo 未监听管道（$($m.pipeStatus)），以日志与 ⑥ 的错误码为证据，跳过探针"
      }
    }
  }

  # ⑤：ca_b 正常创建后 SetSecurityInfo(OWNER) 改属主
  foreach ($k in $owners.Keys) {
    $sid = if ($owners[$k] -eq 'SY') { $SidSystem } elseif ($owners[$k] -eq 'BA') { $SidAdmins } else { $owners[$k] }
    Invoke-ProbeSquat -CaseId 'S-E' -Step "se-e5-$($k.Replace('_', ''))" -As 'ca_b' -PipeName $pipe -Sddl $dacl `
      -SetOwnerSid $sid -Expect 'P6 SetSecurityInfo(OWNER) 失败，记录错误码（预期 1307 ERROR_INVALID_OWNER）' | Out-Null
  }
  # ⑥：ca_b 直接以含伪造属主的 SDDL 创建（S-R9，P6 的主要证据）
  foreach ($k in $owners.Keys) {
    Invoke-ProbeSquat -CaseId 'S-E' -Step "se-e6-$($k.Replace('_', ''))" -As 'ca_b' -PipeName $pipe `
      -Sddl "O:$($owners[$k])$dacl" -Expect 'P6 创建失败，记录错误码（预期 1307 ERROR_INVALID_OWNER）' | Out-Null
  }
  if ((Test-PipePresent $pipe) -eq $true) { Add-SpikeError "S-E 结束后 $pipe 仍存在（squat 进程未退出？）" }
}

function Invoke-CaseF {
  # ca_b 运行改名后的 verge-mihomo.exe，外部控制器 127.0.0.1:19097 并设置 secret；ca_a 只做 tcp-probe
  $secret = New-RandomHex 16
  $script:Scrub.Add($secret)
  $mihomo = @{ Label = 'sf-mihomo'; As = 'ca_b'; ExtCtl = "127.0.0.1:$PortForeignCtl"; Secret = $secret }
  Invoke-WithMihomo 'S-F' $mihomo {
    Invoke-ProbeTcp 'S-F' 'sf-probe-tcp' 'ca_a' $PortForeignCtl '仅记录：GetExtendedTcpTable 条目与 PID、L2 ① ③、L3、WTS pUserSid（预期 ① 与 L3 ACCESS_DENIED，③ 得到 verge-mihomo.exe）' | Out-Null
  }
}

# ---------------------------------------------------------------- 清理

function Invoke-SpikeCleanup {
  foreach ($label in @($script:RunningMihomo)) {
    try { Invoke-Users @{ Action = 'Stop'; Label = $label } | Out-Null } catch { Add-SpikeError "清理：结束 $label 失败：$($_.Exception.Message)" }
  }
  # Cleanup：结束作业与用户名下进程、删除 ca-spike-* 计划任务、撤销 SeBatchLogonRight、删除用户、配置文件与状态目录
  try {
    Invoke-Users @{ Action = 'Cleanup'; UserName = @('ca_a', 'ca_b') } | Out-Null
    if ($LASTEXITCODE -ne 0) { Add-SpikeError "ci-standard-users Cleanup 退出码 $LASTEXITCODE" }
  } catch {
    Add-SpikeError "ci-standard-users Cleanup 失败：$($_.Exception.Message)"
  }
  # SYSTEM 运行的核心不属于 ca_a/ca_b，按映像名兜底
  foreach ($p in @(Get-Process -Name 'verge-mihomo', 'pipe-probe' -ErrorAction SilentlyContinue)) {
    try { Stop-Process -Id $p.Id -Force } catch { Add-SpikeError "清理：结束 $($p.ProcessName)（$($p.Id)）失败：$($_.Exception.Message)" }
  }
  if (Test-Path -LiteralPath $StageDir) {
    try { Remove-Item -LiteralPath $StageDir -Recurse -Force } catch { Add-SpikeError "清理：删除暂存目录失败：$($_.Exception.Message)" }
  }
}

# ---------------------------------------------------------------- main

New-Item -ItemType Directory -Force -Path $ResultsDir | Out-Null
$resolvedResults = (Resolve-Path -LiteralPath $ResultsDir).Path
if ($resolvedResults.StartsWith([IO.Path]::GetFullPath($StateDir), [StringComparison]::OrdinalIgnoreCase)) {
  throw 'ResultsDir 不得位于 ci-standard-users 的状态目录内（其中有 DPAPI 凭据）'
}

$summary = [ordered]@{
  spike      = 'V5 pipe spike（windows-plan §4.1）'
  startedAt  = (Get-Date).ToString('o')
  runner     = [ordered]@{ os = [Environment]::OSVersion.VersionString; ps = $PSVersionTable.PSVersion.ToString() }
  mihomo     = $null
  probe      = [ordered]@{ path = $ProbePath; present = $false; sha256 = $null }
  users      = $null
  methods    = $null
  steps      = $null
  errors     = $null
  note       = '只记录不判定：P1 ~ P7 需人工按 §4.1 判定；探针输出格式待 TODO(probe-cli)'
}
if ($ProbePath -and (Test-Path -LiteralPath $ProbePath)) {
  $summary.probe.present = $true
  $summary.probe.sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $ProbePath).Hash.ToLowerInvariant()
} else {
  Add-SpikeError "探针不存在（$ProbePath），探针步骤记为 probe-missing"
}

try {
  $summary.mihomo = Install-Mihomo
  New-StandardUsers
  $summary.users = @($script:Users.Values | ForEach-Object { [ordered]@{ name = $_.Name; sid = $_.Sid; ownerKey = $_.OwnerKey } })

  # S-A ~ S-E 优先（S-R8），S-F 最后
  foreach ($c in @(${function:Invoke-CaseAB}, ${function:Invoke-CaseC}, ${function:Invoke-CaseD}, ${function:Invoke-CaseE}, ${function:Invoke-CaseF})) {
    try { & $c } catch { Add-SpikeError "用例执行异常：$($_.Exception.Message)" }
  }
} catch {
  Add-SpikeError "准备阶段失败：$($_.Exception.Message)"
} finally {
  Invoke-SpikeCleanup
  $summary.methods = $script:MethodFor
  $summary.steps = $script:Steps.ToArray()
  $summary.errors = $script:Errors.ToArray()
  $summary['finishedAt'] = (Get-Date).ToString('o')
  Write-Json (Join-Path $ResultsDir 'summary.json') $summary
}

if ($script:Errors.Count -gt 0) {
  Write-Info "完成，但有 $($script:Errors.Count) 个错误（见 summary.json）"
  exit 1
}
Write-Info '完成：全部步骤已执行（P1 ~ P7 待人工判定）'
exit 0
