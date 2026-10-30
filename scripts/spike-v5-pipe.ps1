<#
.SYNOPSIS
  V5 pipe spike 编排：windows-plan r4 §4.1，用例 S-A ~ S-F，以及 S-F2（S-F 变体：对比两种启动方式下的进程 / 令牌 DACL）。只存在于 win/v5-pipe-spike 分支，不合并。

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

  探针 CLI 以 spike/pipe-probe/README.md 为准，调用集中在“探针调用层”。每次调用都带 --out，
  写进该作业自己的目录（ACL 仅 SYSTEM、Administrators 与运行身份），由本脚本读取并复制到 ResultsDir；
  判定只看 JSON，退出码照样记录。每条探针记录附 context 自检（self.sid / elevated /
  builtin_admins_enabled）与按 README“用例映射”摘出的 evidence；verdict 一律留空，由人工判定。
  S-E 之前先以 ca_b 做一次环境自检，不满足（非本人 SID、提权或启用 BA）则记错误并跳过 S-E。
  探针不存在时，探针步骤记为 probe-missing，mihomo 与身份部分照常运行，脚本最终以 1 退出。

.PARAMETER ResultsDir
  结果目录（artifact）。不得位于 ci-standard-users 的状态目录内。

.PARAMETER ProbePath
  pipe-probe.exe 的路径。省略或文件不存在时，探针步骤记为 probe-missing。

.PARAMETER Cases
  只运行列出的用例（S-A、S-B、S-C、S-D、S-E、S-F、S-F2、S-D-prime），默认全部。workflow 经 SPIKE_CASES 传入。

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

  [string]$WorkDir,

  # 只运行列出的用例（默认全部）。S-A 与 S-B 共用同一个 mihomo，列出其一即两者都跑
  [ValidateSet('S-A', 'S-B', 'S-C', 'S-D', 'S-E', 'S-F', 'S-F2', 'S-D-prime')]
  [string[]]$Cases = @('S-A', 'S-B', 'S-C', 'S-D', 'S-E', 'S-F', 'S-F2', 'S-D-prime')
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
$PortF2Credential = 19096  # S-F2：ca_b 核心经 Credential 启动
$PortF2Task = 19095        # S-F2：ca_b 核心经 ScheduledTask 启动
$ListenWaitSec = 30
$ProbeTimeoutSec = 60
$PipeGoneWaitSec = 10
$SquatHoldSec = 30         # README“P7 备选”：squat --hold 的秒数

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
  ConvertTo-Json -InputObject $Value -Depth 16 | Set-Content -LiteralPath $Path -Encoding UTF8
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

function Get-MethodParam([string]$As, [string]$Force) {
  if ($Force) { return @{ Method = $Force } }
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
  foreach ($f in @('identity.txt', 'stdout.txt', 'stderr.txt', 'exitcode.txt', 'wrapper-error.txt', 'job.json', 'probe.json', 'ready.json', 'open-access.json')) {
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

function Remember-Method([string]$As, [string]$Label, [string]$Forced) {
  $job = Read-JobRecord $Label
  if ($job -and $job.Method -and -not $Forced -and -not $script:MethodFor.ContainsKey($As)) {
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
    [string]$PipeName, [string]$Sddl, [string]$ExtCtl, [string]$Secret,
    [string]$Method   # 非空时强制启动方式（S-F2），且不改变该身份记住的方式
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
  } + (Get-MethodParam $As $Method)
  $script:RunningMihomo.Add($Label)
  Invoke-Users $params | Out-Null
  $rec.method = Remember-Method $As $Label $Method
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
  $status = if ($Record.Contains('status')) { $Record.status } elseif ($Record.Contains('error') -and $Record.error) { 'error' } else { 'ran' }
  $script:Steps.Add([ordered]@{ case = $CaseId; step = $Step; status = $status; file = "$CaseId/$Step.json" })
}

# ---------------------------------------------------------------- 探针 JSON 读取
# 探针省略缺省字段（README“Step”表），StrictMode 下不能直接点属性，统一经 Get-Field 读取。

function Get-Field($Obj, [string]$Path) {
  $cur = $Obj
  foreach ($seg in $Path.Split('.')) {
    if ($null -eq $cur) { return $null }
    $p = $cur.PSObject.Properties[$seg]
    if ($null -eq $p) { return $null }
    $cur = $p.Value
  }
  if ($cur -is [array]) { return , $cur }   # 不让单元素 / 空数组在返回时被展开
  return $cur
}

function Get-ProbeSteps($Report, [string]$Name) {
  $steps = Get-Field $Report 'steps'
  if ($null -eq $steps) { return @() }
  return @($steps | Where-Object { (Get-Field $_ 'step') -eq $Name })
}

function Get-ProbeStep($Report, [string]$Name) {
  $all = @(Get-ProbeSteps $Report $Name)
  if ($all.Count -eq 0) { return $null }
  return $all[0]
}

function Get-StepBrief($Step) {
  # 只摘调用结果；完整内容在同一份记录的 output 中
  if ($null -eq $Step) { return $null }
  return [ordered]@{
    ok               = Get-Field $Step 'ok'
    win32_error      = Get-Field $Step 'win32_error'
    win32_error_name = Get-Field $Step 'win32_error_name'
    api              = Get-Field $Step 'api'
    error            = Get-Field $Step 'error'
  }
}

function Test-ProbeContext($Report, [string]$As) {
  # README“环境自检”：ca_a/ca_b 运行的 JSON 中 self.sid 为本人，elevated 与 builtin_admins_enabled 均为 false
  $expected = if ($script:Users.ContainsKey($As)) { $script:Users[$As].Sid } else { $null }
  $selfSid = Get-Field $Report 'context.self.sid'
  $elevated = Get-Field $Report 'context.elevated'
  $admins = Get-Field $Report 'context.builtin_admins_enabled'
  $sidOk = ($null -ne $expected) -and ($selfSid -eq $expected)
  return [ordered]@{
    selfSid              = $selfSid
    expectedSid          = $expected
    selfSidMatches       = $sidOk
    elevated             = $elevated
    builtinAdminsEnabled = $admins
    ok                   = ($sidOk -and ($elevated -eq $false) -and ($admins -eq $false))
  }
}

# ---------------------------------------------------------------- 探针（通用执行）

function Get-ProbeOutPath([string]$Label) {
  # --out 写进该作业自己的目录：ci-standard-users 已把它的 ACL 设为仅 SYSTEM、Administrators
  # 与运行身份可写（最小权限，另一个标准用户无权访问），runner 负责收集
  return (Join-Path (Get-JobDir $Label) 'probe.json')
}

function New-ProbeRecord([string]$CaseId, [string]$Step, [string]$As, [string[]]$ProbeArgs, [string]$Expect) {
  return [ordered]@{
    case = $CaseId; step = $Step; kind = 'probe'; as = $As; args = $ProbeArgs; expect = $Expect
    status = $null; method = $null; forcedMethod = $null; exitCode = $null; timedOut = $false
    output = $null; outputSource = $null; context = $null; evidence = $null; identity = $null; error = $null
    verdict = $null   # 只记录；P1 ~ P7 由人工判定
  }
}

function Complete-ProbeRecord($Rec, [scriptblock]$Evidence) {
  # 判定只看 JSON，不看退出码（README）；退出码照样记录。优先读 --out，拿不到再看 stdout。
  $step = $Rec.step
  $rec.method = Remember-Method $Rec.as $step $Rec.forcedMethod
  $Rec.identity = Get-IdentityInfo $step
  $report = $null
  foreach ($src in @(@{ n = 'out-file'; p = (Get-ProbeOutPath $step) }, @{ n = 'stdout'; p = (Join-Path (Get-JobDir $step) 'stdout.txt') })) {
    $text = Read-SharedText $src.p
    if (-not $text) { continue }
    try {
      $report = $text | ConvertFrom-Json
      $Rec.outputSource = $src.n
      break
    } catch {
      $Rec.error = "$($src.n) 不是合法 JSON"
    }
  }
  switch ($Rec.exitCode) {
    1 { $Rec.error = '探针退出码 1：写 --out 失败（JSON 只在 stdout）' }
    2 { $Rec.status = 'usage-error'; $Rec.error = '探针退出码 2：命令行错误（CLI 接线有误）' }
    3 { $Rec.status = 'error'; $Rec.error = '探针退出码 3：非 Windows 平台' }
  }
  if ($null -ne $report) {
    $Rec.output = $report
    $Rec.context = Test-ProbeContext $report $Rec.as
    if (-not $Rec.context.ok) { Add-SpikeError "$($Rec.case)/$step：环境自检不满足（self.sid / elevated / builtin_admins_enabled），结果不具代表性" }
    if ($Evidence) {
      try { $Rec.evidence = & $Evidence $report } catch { $Rec.evidence = [ordered]@{ error = $_.Exception.Message } }
    }
  } elseif ($Rec.status -eq 'ran' -or $Rec.status -eq 'timeout') {
    $Rec.status = 'no-json'
    if (-not $Rec.error) { $Rec.error = '探针未产生 JSON（--out 与 stdout 均为空）' }
  }
  Save-JobOutput $step (Join-Path $ResultsDir "$($Rec.case)\$step")
  Add-Step $Rec.case $step $Rec
  if ($Rec.status -in @('error', 'usage-error', 'no-json')) { Add-SpikeError "$($Rec.case)/$step：$($Rec.error)" }
  return $Rec
}

function Invoke-Probe {
  param([string]$CaseId, [string]$Step, [string]$As, [string[]]$ProbeArgs, [string]$Expect, [scriptblock]$Evidence, [string]$Method)
  $fullArgs = @($ProbeArgs) + @('--out', (Get-ProbeOutPath $Step))
  $rec = New-ProbeRecord $CaseId $Step $As $fullArgs $Expect
  $rec.forcedMethod = if ($Method) { $Method } else { $null }
  if (-not $ProbePath -or -not (Test-Path -LiteralPath $ProbePath)) {
    $rec.status = 'probe-missing'
    Add-Step $CaseId $Step $rec
    return $rec
  }
  $params = @{
    Action = 'Run'; As = $As; Label = $Step
    StagePath = @($ProbePath); FilePath = (Split-Path -Leaf $ProbePath)
    ArgumentList = $fullArgs; TimeoutSec = $ProbeTimeoutSec
  } + (Get-MethodParam $As $Method)
  try {
    Invoke-Users $params | Out-Null
    $rec.exitCode = $LASTEXITCODE
    $rec.timedOut = ($rec.exitCode -eq 124)
    $rec.status = if ($rec.timedOut) { 'timeout' } else { 'ran' }
  } catch {
    $rec.status = 'error'
    $rec.error = $_.Exception.Message
  }
  return (Complete-ProbeRecord $rec $Evidence)
}

# ---------------------------------------------------------------- 探针调用层（CLI 契约：spike/pipe-probe/README.md）

function Invoke-ProbePipe {
  # pipe-probe pipe <name> [--timeout-ms <ms>] [--out <file>]
  param([string]$CaseId, [string]$Step, [string]$As, [string]$PipeName, [string]$Expect, [scriptblock]$Evidence, [int]$TimeoutMs = 5000)
  return (Invoke-Probe -CaseId $CaseId -Step $Step -As $As -Expect $Expect -Evidence $Evidence `
      -ProbeArgs @('pipe', $PipeName, '--timeout-ms', [string]$TimeoutMs))
}

function Invoke-ProbeTcp {
  # pipe-probe tcp-probe <port> [--out <file>]（不建立连接、不发请求）
  param([string]$CaseId, [string]$Step, [string]$As, [int]$Port, [string]$Expect, [scriptblock]$Evidence, [string]$Method)
  return (Invoke-Probe -CaseId $CaseId -Step $Step -As $As -Expect $Expect -Evidence $Evidence -Method $Method `
      -ProbeArgs @('tcp-probe', [string]$Port))
}

function Invoke-ProbeSquat {
  # pipe-probe squat <name> <sddl|-> [--set-owner <sid|SY|BA>]... [--out <file>]（不 hold：创建 / 改属主后立即退出）
  param([string]$CaseId, [string]$Step, [string]$As, [string]$PipeName, [string]$Sddl, [string[]]$SetOwner = @(), [string]$Expect, [scriptblock]$Evidence)
  $probeArgs = @('squat', $PipeName, $Sddl)
  foreach ($o in $SetOwner) { $probeArgs += @('--set-owner', $o) }
  return (Invoke-Probe -CaseId $CaseId -Step $Step -As $As -Expect $Expect -Evidence $Evidence -ProbeArgs $probeArgs)
}

function Start-ProbeSquatHold {
  # pipe-probe squat <name> <sddl> --hold <secs> --ready-file <file> --out <file>：后台运行，返回 ready 文件路径
  param([string]$Step, [string]$As, [string]$PipeName, [string]$Sddl, [int]$HoldSec)
  $ready = Join-Path (Get-JobDir $Step) 'ready.json'
  $probeArgs = @('squat', $PipeName, $Sddl, '--hold', [string]$HoldSec, '--ready-file', $ready, '--out', (Get-ProbeOutPath $Step))
  $params = @{
    Action = 'Start'; As = $As; Label = $Step
    StagePath = @($ProbePath); FilePath = (Split-Path -Leaf $ProbePath)
    ArgumentList = $probeArgs
  } + (Get-MethodParam $As)
  Invoke-Users $params | Out-Null
  return [ordered]@{ args = $probeArgs; ready = $ready }
}

# ---------------------------------------------------------------- 证据摘录（事实，不是判定）

function Get-PipeOwnerSid($Report) { return (Get-Field (Get-ProbeStep $Report 'owner') 'value.owner.sid') }

function Get-ForgeTargets {
  return @($script:Users['ca_a'].Sid, $SidSystem, $SidAdmins)
}

function Get-TcpEvidence($Report) {
  $pidValue = Get-Field (Get-ProbeStep $Report 'listener_pids') 'value'
  $pids = @()
  if ($null -ne $pidValue) { $pids = @($pidValue) }
  $perPid = foreach ($procId in $pids) {
    $pick = { param($name) @(Get-ProbeSteps $Report $name | Where-Object { (Get-Field $_ 'pid') -eq $procId }) | Select-Object -First 1 }
    [ordered]@{
      pid          = $procId
      l2_1         = Get-StepBrief (& $pick 'l2_1')
      l2_1_file    = Get-Field (& $pick 'l2_1') 'value.file_name'
      l2_2         = Get-StepBrief (& $pick 'l2_2')
      l2_3         = Get-StepBrief (& $pick 'l2_3')
      l2_3_exe     = Get-Field (& $pick 'l2_3') 'value.exe_file'
      l3           = Get-StepBrief (& $pick 'l3')
      l3_user_sid  = Get-Field (& $pick 'l3') 'value.token_user.sid'
      wts_user_sid = Get-Field (& $pick 'wts') 'value.user_sid.sid'
    }
  }
  return [ordered]@{
    tcp_v4_entries = Get-Field (Get-ProbeStep $Report 'tcp_table_v4') 'value.entries'
    tcp_v6_entries = Get-Field (Get-ProbeStep $Report 'tcp_table_v6') 'value.entries'
    listener_pids  = $pids
    per_pid        = @($perPid)
  }
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
    Invoke-ProbePipe -CaseId 'S-A' -Step 'sa-probe' -As 'ca_a' -PipeName $pipe `
      -Expect 'P1 http_version.status == 200；P2 owner.sid == ca_a；P3 l2_1.file_name == verge-mihomo.exe' -Evidence {
      param($r)
      $l21 = Get-ProbeStep $r 'l2_1'
      [ordered]@{
        P1_http_status     = Get-Field (Get-ProbeStep $r 'http_version') 'value.status'
        P2_owner_sid       = Get-PipeOwnerSid $r
        P2_owner_is_ca_a   = ((Get-PipeOwnerSid $r) -eq $script:Users['ca_a'].Sid)
        P3_l2_1            = Get-StepBrief $l21
        P3_l2_1_file_match = ([string](Get-Field $l21 'value.file_name') -ieq 'verge-mihomo.exe')
      }
    } | Out-Null
    Invoke-ProbePipe -CaseId 'S-B' -Step 'sb-probe' -As 'ca_b' -PipeName $pipe `
      -Expect 'P4 open.ok == false && open.win32_error == 5' -Evidence {
      param($r)
      $open = Get-ProbeStep $r 'open'
      [ordered]@{ P4_open = Get-StepBrief $open; P4_access_denied = ((Get-Field $open 'ok') -eq $false -and (Get-Field $open 'win32_error') -eq 5) }
    } | Out-Null
  }
}

function Invoke-CaseC {
  $a = $script:Users['ca_a']
  $pipe = "${PipePrefix}verge-mihomo-production-$($a.OwnerKey)"
  $mihomo = @{ Label = 'sc-mihomo'; As = 'SYSTEM'; PipeName = $pipe; Sddl = (Get-VergeSddl $a.Sid); ExtCtl = "127.0.0.1:$PortServiceCtl" }
  Invoke-WithMihomo 'S-C' $mihomo {
    Invoke-ProbePipe -CaseId 'S-C' -Step 'sc-probe-pipe' -As 'ca_a' -PipeName $pipe `
      -Expect 'P5 http_version.status == 200 且 owner 为 SYSTEM 或 BA；记录 l2_1/l2_2/l2_3/l3/wts（预期 l2_1 = 5，② 或 ③ ok）' -Evidence {
      param($r)
      $owner = Get-ProbeStep $r 'owner'
      [ordered]@{
        P5_http_status          = Get-Field (Get-ProbeStep $r 'http_version') 'value.status'
        P5_owner_sid            = Get-PipeOwnerSid $r
        P5_owner_is_system      = Get-Field $owner 'value.owner.is_system'
        P5_owner_is_builtin_admins = Get-Field $owner 'value.owner.is_builtin_admins'
        l2_1                    = Get-StepBrief (Get-ProbeStep $r 'l2_1')
        l2_2                    = Get-StepBrief (Get-ProbeStep $r 'l2_2')
        l2_3                    = Get-StepBrief (Get-ProbeStep $r 'l2_3')
        l3                      = Get-StepBrief (Get-ProbeStep $r 'l3')
        wts_user_sid            = Get-Field (Get-ProbeStep $r 'wts') 'value.user_sid.sid'
      }
    } | Out-Null
    Invoke-ProbeTcp -CaseId 'S-C' -Step 'sc-probe-tcp' -As 'ca_a' -Port $PortServiceCtl `
      -Expect '按 2.1 规则 4 应为 listener-unverified（C-4，如 l3.win32_error == 5）；WTS 只记录' -Evidence {
      param($r) Get-TcpEvidence $r
    } | Out-Null
  }
}

function Invoke-CaseD {
  $a = $script:Users['ca_a']
  $pipe = "${PipePrefix}verge-mihomo-sidecar-release-$($a.OwnerKey)"
  # 不设 LISTEN_NAMEDPIPE_SDDL：mihomo 默认 D:PAI(A;OICI;GWGR;;;BU)(A;OICI;GWGR;;;SY)
  Invoke-WithMihomo 'S-D' @{ Label = 'sd-mihomo'; As = 'ca_a'; PipeName = $pipe } {
    Invoke-ProbePipe -CaseId 'S-D' -Step 'sd-probe' -As 'ca_b' -PipeName $pipe `
      -Expect '仅记录：open 结果；连上时 dacl.aces（预期 BU 的 ACE has_write_bits == true）与 dacl.sddl' -Evidence {
      param($r)
      $dacl = Get-ProbeStep $r 'dacl'
      [ordered]@{ open = Get-StepBrief (Get-ProbeStep $r 'open'); dacl_sddl = Get-Field $dacl 'value.sddl'; dacl_aces = Get-Field $dacl 'value.aces' }
    } | Out-Null
  }
}

function Invoke-CaseE {
  $a = $script:Users['ca_a']
  $pipe = "${PipePrefix}verge-mihomo-sidecar-release-$($a.OwnerKey)"
  $dacl = 'D:(A;;GA;;;WD)'

  # P7 / S-E 前提：ca_b 的令牌不得提权、不得启用 BA（README“环境自检”）。用一个不存在的管道名取 context。
  $envPipe = "${PipePrefix}verge-mihomo-spike-envcheck-$(New-RandomHex 8)"
  $envRec = Invoke-ProbePipe -CaseId 'S-E' -Step 'se-env-cab' -As 'ca_b' -PipeName $envPipe -TimeoutMs 1000 `
    -Expect '环境自检：context.self.sid == ca_b，elevated == false，builtin_admins_enabled == false'
  if ($null -eq $envRec.context -or -not $envRec.context.ok) {
    Add-SpikeError "S-E 环境自检失败（ca_b 的 context：$(ConvertTo-Json -InputObject $envRec.context -Compress)），S-E 结果不具代表性，跳过 S-E"
    return
  }

  # ①②③：ca_b 用真实 mihomo 以伪造属主抢注；④：不带 O:（P7）。不论 mihomo 是否监听成功都让 ca_a 探测（README）。
  $mihomoTries = @(
    @{ n = 'e1'; sddl = "O:$($a.Sid)$dacl"; p7 = $false }
    @{ n = 'e2'; sddl = "O:SY$dacl"; p7 = $false }
    @{ n = 'e3'; sddl = "O:BA$dacl"; p7 = $false }
    @{ n = 'e4'; sddl = $dacl; p7 = $true }
  )
  foreach ($t in $mihomoTries) {
    Invoke-WithMihomo 'S-E' @{ Label = "se-$($t.n)-mihomo"; As = 'ca_b'; PipeName = $pipe; Sddl = $t.sddl } {
      param($m)
      if ($t.p7) {
        Invoke-ProbePipe -CaseId 'S-E' -Step "se-$($t.n)-probe" -As 'ca_a' -PipeName $pipe `
          -Expect 'P7 open.ok && owner.sid == ca_b' -Evidence {
          param($r)
          [ordered]@{ P7_open = Get-StepBrief (Get-ProbeStep $r 'open'); P7_owner_sid = Get-PipeOwnerSid $r; P7_owner_is_ca_b = ((Get-PipeOwnerSid $r) -eq $script:Users['ca_b'].Sid) }
        } | Out-Null
      } else {
        Invoke-ProbePipe -CaseId 'S-E' -Step "se-$($t.n)-probe" -As 'ca_a' -PipeName $pipe `
          -Expect 'P6（旁证）：open 失败，或 owner.sid 不在 {ca_a, SYSTEM, BA}' -Evidence {
          param($r)
          $owner = Get-PipeOwnerSid $r
          [ordered]@{
            mihomo_pipe_status = $m.pipeStatus
            P6_open            = Get-StepBrief (Get-ProbeStep $r 'open')
            P6_owner_sid       = $owner
            P6_owner_forged    = (($null -ne $owner) -and ((Get-ForgeTargets) -contains $owner))
          }
        } | Out-Null
      }
    }
  }

  # ⑤：一次调用，正常创建后依次 SetSecurityInfo(OWNER) 为 ca_a、SY、BA
  Invoke-ProbeSquat -CaseId 'S-E' -Step 'se-e5-squat' -As 'ca_b' -PipeName $pipe -Sddl '-' -SetOwner @($a.Sid, 'SY', 'BA') `
    -Expect 'P6（主证据）：三个 set_owner 均 ok == false、win32_error == 1307；每个 owner_readback.sid == ca_b' -Evidence {
    param($r)
    $sets = @(Get-ProbeSteps $r 'set_owner' | ForEach-Object { [ordered]@{ target = Get-Field $_ 'target'; ok = Get-Field $_ 'ok'; win32_error = Get-Field $_ 'win32_error' } })
    $reads = @(Get-ProbeSteps $r 'owner_readback' | ForEach-Object { [ordered]@{ target = Get-Field $_ 'target'; owner_sid = Get-Field $_ 'value.owner.sid' } })
    $afterSet = @($reads | Where-Object { $null -ne $_.target })
    [ordered]@{
      create                       = Get-StepBrief (Get-ProbeStep $r 'create')
      P6_set_owner                 = $sets
      P6_set_owner_count           = $sets.Count
      P6_all_set_owner_1307        = (($sets.Count -eq 3) -and (@($sets | Where-Object { $_.ok -ne $false -or $_.win32_error -ne 1307 }).Count -eq 0))
      owner_readback               = $reads
      P6_readbacks_after_set_ca_b  = (($afterSet.Count -eq 3) -and (@($afterSet | Where-Object { $_.owner_sid -ne $script:Users['ca_b'].Sid }).Count -eq 0))
    }
  } | Out-Null

  # ⑥：三次独立调用，直接以含伪造属主的 SDDL 创建（S-R9，P6 的主要证据）
  $sixth = [ordered]@{ caa = $a.Sid; sy = 'SY'; ba = 'BA' }
  foreach ($k in $sixth.Keys) {
    $targetSid = switch ($k) { 'caa' { $a.Sid } 'sy' { $SidSystem } 'ba' { $SidAdmins } }
    Invoke-ProbeSquat -CaseId 'S-E' -Step "se-e6-$k" -As 'ca_b' -PipeName $pipe -Sddl "O:$($sixth[$k])$dacl" `
      -Expect 'P6（主证据）：create.ok == false && create.win32_error == 1307；若 create 成功且 owner_readback == 目标即伪造成功（no-go）' -Evidence {
      param($r)
      $create = Get-ProbeStep $r 'create'
      $readback = Get-Field (Get-ProbeStep $r 'owner_readback') 'value.owner.sid'
      [ordered]@{
        target_sid          = $targetSid
        P6_create           = Get-StepBrief $create
        P6_create_1307      = ((Get-Field $create 'ok') -eq $false -and (Get-Field $create 'win32_error') -eq 1307)
        owner_readback_sid  = $readback
        P6_forged           = (($null -ne $readback) -and ($readback -eq $targetSid))
      }
    } | Out-Null
  }

  Invoke-P7Squat $pipe $dacl
  if (-not (Wait-PipeGone $pipe)) { Add-SpikeError "S-E 结束后 $pipe 仍存在（squat 进程未退出？）" }
}

function Invoke-P7Squat([string]$Pipe, [string]$Dacl) {
  # README“P7 备选（无需 mihomo）”：ca_b squat --hold，等 --ready-file 出现后 ca_a 探测；应看到 503 + {"pipe_probe":"squat"}
  $squatStep = 'se-p7-squat'
  $squatRec = New-ProbeRecord 'S-E' $squatStep 'ca_b' @() 'P7 备选：hold 期间有对应的 client_connected'
  if (-not $ProbePath -or -not (Test-Path -LiteralPath $ProbePath)) {
    $squatRec.status = 'probe-missing'
    Add-Step 'S-E' $squatStep $squatRec
    return
  }
  try {
    $started = Start-ProbeSquatHold -Step $squatStep -As 'ca_b' -PipeName $Pipe -Sddl $Dacl -HoldSec $SquatHoldSec
    $squatRec.args = $started.args
    $squatRec.status = 'ran'
    $deadline = (Get-Date).AddSeconds($ListenWaitSec)
    while ((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath $started.ready)) { Start-Sleep -Milliseconds 250 }
    if (Test-Path -LiteralPath $started.ready) {
      Invoke-ProbePipe -CaseId 'S-E' -Step 'se-p7-probe' -As 'ca_a' -PipeName $Pipe `
        -Expect 'P7 备选：owner.sid == ca_b；http_version.status == 503 且 body_json.pipe_probe == squat' -Evidence {
        param($r)
        $http = Get-ProbeStep $r 'http_version'
        [ordered]@{
          P7_owner_sid      = Get-PipeOwnerSid $r
          P7_owner_is_ca_b  = ((Get-PipeOwnerSid $r) -eq $script:Users['ca_b'].Sid)
          http_status       = Get-Field $http 'value.status'
          body_pipe_probe   = Get-Field $http 'value.body_json.pipe_probe'
          reached_squatter  = ((Get-Field $http 'value.status') -eq 503 -and (Get-Field $http 'value.body_json.pipe_probe') -eq 'squat')
        }
      } | Out-Null
    } else {
      $squatRec.error = "$ListenWaitSec 秒内没有 ready 文件"
    }
    # 等 hold 自然结束（complete: true）；超时再强制结束——--out 有检查点，证据不丢
    if (-not (Test-Path -LiteralPath (Join-Path (Get-JobDir $squatStep) 'exitcode.txt'))) {
      $deadline = (Get-Date).AddSeconds($SquatHoldSec + 15)
      while ((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath (Join-Path (Get-JobDir $squatStep) 'exitcode.txt'))) { Start-Sleep -Milliseconds 500 }
    }
    $exitFile = Join-Path (Get-JobDir $squatStep) 'exitcode.txt'
    if (Test-Path -LiteralPath $exitFile) {
      $squatRec.exitCode = [int]((Read-SharedText $exitFile).Trim())
    } else {
      $squatRec.timedOut = $true
      Invoke-Users @{ Action = 'Stop'; Label = $squatStep } | Out-Null
    }
  } catch {
    $squatRec.status = 'error'
    $squatRec.error = $_.Exception.Message
  }
  Complete-ProbeRecord $squatRec {
    param($r)
    [ordered]@{
      complete          = Get-Field $r 'complete'
      ready_file        = Get-StepBrief (Get-ProbeStep $r 'ready_file')
      client_connected  = @(Get-ProbeSteps $r 'client_connected' | ForEach-Object { Get-Field $_ 'value' })
    }
  } | Out-Null
  if ($squatRec.error -and $squatRec.status -ne 'error') { Add-SpikeError "S-E/$squatStep：$($squatRec.error)" }
}

function Invoke-CaseF {
  # ca_b 运行改名后的 verge-mihomo.exe，外部控制器 127.0.0.1:19097 并设置 secret；ca_a 只做 tcp-probe
  $secret = New-RandomHex 16
  $script:Scrub.Add($secret)
  $mihomo = @{ Label = 'sf-mihomo'; As = 'ca_b'; ExtCtl = "127.0.0.1:$PortForeignCtl"; Secret = $secret }
  Invoke-WithMihomo 'S-F' $mihomo {
    Invoke-ProbeTcp -CaseId 'S-F' -Step 'sf-probe-tcp' -As 'ca_a' -Port $PortForeignCtl `
      -Expect '仅记录：tcp_table、listener_pids、该 PID 的 l2_1 / l2_3 / l3、wts.user_sid（是否 == ca_b）' -Evidence {
      param($r)
      $e = Get-TcpEvidence $r
      $e['wts_user_sid_is_ca_b'] = @($e.per_pid | ForEach-Object { $_.wts_user_sid -eq $script:Users['ca_b'].Sid })
      $e
    } | Out-Null
  }
}

# ---------------------------------------------------------------- S-F2：进程 / 令牌安全信息（Pit 结果 V1）
# S-F / S-D 中 ca_a 与 ca_b 能互相 OpenProcess（L2 ①）并读令牌（L3），与 §4.1 预期的 ACCESS_DENIED 相反。
# S-F2 = S-F 的变体：ca_b 的 verge-mihomo.exe 分别经 Credential（seclogon）与 ScheduledTask（LogonType Password）
# 启动，ca_a 的 tcp-probe 也分别经两种方式运行；runner 管理员（编排脚本本身）读取目标进程与令牌的
# 对象 DACL、令牌组（含 logon SID 及属性）、完整性级别、登录会话类型，作为解释依据。只记录，不判定。

$ProcSecSource = @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;

public static class CaSpikeProcSec {
  [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
  [DllImport("kernel32.dll", SetLastError = true)] public static extern bool CloseHandle(IntPtr h);
  [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
  [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr p);
  [DllImport("advapi32.dll", SetLastError = true)] static extern bool OpenProcessToken(IntPtr proc, uint access, out IntPtr token);
  [DllImport("advapi32.dll")] static extern uint GetSecurityInfo(IntPtr h, int type, uint info, IntPtr owner, IntPtr group, IntPtr dacl, IntPtr sacl, out IntPtr sd);
  [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
  static extern bool ConvertSecurityDescriptorToStringSecurityDescriptorW(IntPtr sd, uint rev, uint info, out IntPtr str, out uint len);
  [DllImport("advapi32.dll", SetLastError = true)] static extern bool GetTokenInformation(IntPtr token, int cls, IntPtr buf, int len, out int ret);
  [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)] static extern bool LookupPrivilegeValueW(string sys, string name, out long luid);
  [DllImport("advapi32.dll", SetLastError = true)] static extern bool AdjustTokenPrivileges(IntPtr token, bool disableAll, ref TokenPrivileges state, int len, IntPtr prev, IntPtr ret);
  [DllImport("secur32.dll")] static extern uint LsaGetLogonSessionData(ref long luid, out IntPtr data);
  [DllImport("secur32.dll")] static extern uint LsaFreeReturnBuffer(IntPtr p);

  [StructLayout(LayoutKind.Sequential, Pack = 4)] struct TokenPrivileges { public int Count; public long Luid; public uint Attributes; }
  [StructLayout(LayoutKind.Sequential)] struct LsaString { public ushort Length; public ushort MaximumLength; public IntPtr Buffer; }
  [StructLayout(LayoutKind.Sequential)] struct LogonSessionData {
    public uint Size; public uint LogonIdLow; public int LogonIdHigh;
    public LsaString UserName; public LsaString LogonDomain; public LsaString AuthenticationPackage;
    public uint LogonType; public uint Session;
  }

  // 返回 GetLastError；0 = 已启用（AdjustTokenPrivileges 成功但未分配时为 1300）
  public static int EnableDebugPrivilege() {
    IntPtr tok;
    if (!OpenProcessToken(GetCurrentProcess(), 0x28 /* ADJUST_PRIVILEGES | QUERY */, out tok)) return Marshal.GetLastWin32Error();
    try {
      long luid;
      if (!LookupPrivilegeValueW(null, "SeDebugPrivilege", out luid)) return Marshal.GetLastWin32Error();
      TokenPrivileges tp = new TokenPrivileges { Count = 1, Luid = luid, Attributes = 2 /* SE_PRIVILEGE_ENABLED */ };
      if (!AdjustTokenPrivileges(tok, false, ref tp, 0, IntPtr.Zero, IntPtr.Zero)) return Marshal.GetLastWin32Error();
      return Marshal.GetLastWin32Error();
    } finally { CloseHandle(tok); }
  }

  public static IntPtr OpenProc(int pid, uint access) {
    IntPtr h = OpenProcess(access, false, pid);
    if (h == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "OpenProcess");
    return h;
  }

  public static IntPtr OpenToken(IntPtr proc, uint access) {
    IntPtr t;
    if (!OpenProcessToken(proc, access, out t)) throw new Win32Exception(Marshal.GetLastWin32Error(), "OpenProcessToken");
    return t;
  }

  // 对象安全描述符（OWNER | GROUP | DACL | LABEL）→ SDDL
  public static string GetSddl(IntPtr h) {
    IntPtr sd;
    uint info = 0x1 | 0x2 | 0x4 | 0x10;
    uint rc = GetSecurityInfo(h, 6 /* SE_KERNEL_OBJECT */, info, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, out sd);
    if (rc != 0) throw new Win32Exception((int)rc, "GetSecurityInfo");
    try {
      IntPtr str; uint len;
      if (!ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, 1, info, out str, out len))
        throw new Win32Exception(Marshal.GetLastWin32Error(), "ConvertSecurityDescriptorToStringSecurityDescriptorW");
      try { return Marshal.PtrToStringUni(str); } finally { LocalFree(str); }
    } finally { LocalFree(sd); }
  }

  static IntPtr Info(IntPtr token, int cls) {
    int len;
    GetTokenInformation(token, cls, IntPtr.Zero, 0, out len);
    if (len <= 0) throw new Win32Exception(Marshal.GetLastWin32Error(), "GetTokenInformation(" + cls + ")");
    IntPtr buf = Marshal.AllocHGlobal(len);
    if (!GetTokenInformation(token, cls, buf, len, out len)) {
      int err = Marshal.GetLastWin32Error();
      Marshal.FreeHGlobal(buf);
      throw new Win32Exception(err, "GetTokenInformation(" + cls + ")");
    }
    return buf;
  }

  // "SID|属性(十六进制)" 列表：TokenUser(1) / TokenGroups(2) / TokenIntegrityLevel(25)
  public static string[] SidAndAttributes(IntPtr token, int cls) {
    IntPtr buf = Info(token, cls);
    try {
      int count = 1, offset = 0;
      if (cls == 2) { count = Marshal.ReadInt32(buf); offset = IntPtr.Size; }
      string[] r = new string[count];
      int size = IntPtr.Size * 2;
      for (int i = 0; i < count; i++) {
        IntPtr psid = Marshal.ReadIntPtr(buf, offset + i * size);
        uint attr = (uint)Marshal.ReadInt32(buf, offset + i * size + IntPtr.Size);
        r[i] = new SecurityIdentifier(psid).Value + "|" + attr.ToString("X8");
      }
      return r;
    } finally { Marshal.FreeHGlobal(buf); }
  }

  public static uint ReadUInt32(IntPtr token, int cls, int offset) {
    IntPtr buf = Info(token, cls);
    try { return (uint)Marshal.ReadInt32(buf, offset); } finally { Marshal.FreeHGlobal(buf); }
  }

  public static long ReadInt64(IntPtr token, int cls, int offset) {
    IntPtr buf = Info(token, cls);
    try { return Marshal.ReadInt64(buf, offset); } finally { Marshal.FreeHGlobal(buf); }
  }

  // TokenDefaultDacl(6)：返回 ACL 原始字节（可能为空 = 无默认 DACL）
  public static byte[] DefaultDacl(IntPtr token) {
    IntPtr buf = Info(token, 6);
    try {
      IntPtr acl = Marshal.ReadIntPtr(buf);
      if (acl == IntPtr.Zero) return new byte[0];
      int size = (ushort)Marshal.ReadInt16(acl, 2);
      byte[] bytes = new byte[size];
      Marshal.Copy(acl, bytes, 0, size);
      return bytes;
    } finally { Marshal.FreeHGlobal(buf); }
  }

  // "LogonType|Session|AuthenticationPackage|Domain\\User"
  public static string LogonSession(long luid) {
    IntPtr data;
    uint rc = LsaGetLogonSessionData(ref luid, out data);
    if (rc != 0) throw new Win32Exception(unchecked((int)rc), "LsaGetLogonSessionData NTSTATUS 0x" + rc.ToString("X8"));
    try {
      LogonSessionData d = (LogonSessionData)Marshal.PtrToStructure(data, typeof(LogonSessionData));
      Func<LsaString, string> s = x => x.Buffer == IntPtr.Zero ? "" : Marshal.PtrToStringUni(x.Buffer, x.Length / 2);
      return d.LogonType + "|" + d.Session + "|" + s(d.AuthenticationPackage) + "|" + s(d.LogonDomain) + "\\" + s(d.UserName);
    } finally { LsaFreeReturnBuffer(data); }
  }
}
'@

$LogonTypeNames = @{
  2 = 'Interactive'; 3 = 'Network'; 4 = 'Batch'; 5 = 'Service'; 7 = 'Unlock'; 8 = 'NetworkCleartext'
  9 = 'NewCredentials'; 10 = 'RemoteInteractive'; 11 = 'CachedInteractive'
}

function Initialize-ProcSec {
  if (-not ('CaSpikeProcSec' -as [type])) { Add-Type -TypeDefinition $ProcSecSource -Language CSharp }
}

function Get-SidName([string]$Sid) {
  if ($Sid -match '^S-1-5-5-\d+-\d+$') { return 'LogonSessionId' }
  try { return ([Security.Principal.SecurityIdentifier]::new($Sid)).Translate([Security.Principal.NTAccount]).Value } catch { return $null }
}

function ConvertFrom-GroupAttributes([uint32]$Attr) {
  $flags = foreach ($f in @(
      @(0x1, 'Mandatory'), @(0x2, 'EnabledByDefault'), @(0x4, 'Enabled'), @(0x8, 'Owner'), @(0x10, 'UseForDenyOnly'),
      @(0x20, 'Integrity'), @(0x40, 'IntegrityEnabled'), @(0x20000000, 'Resource'), @(0xC0000000u, 'LogonId'))) {
    if (($Attr -band [uint32]$f[0]) -eq [uint32]$f[0]) { $f[1] }
  }
  return @($flags)
}

function ConvertFrom-SidAttrList([string[]]$Items) {
  return @($Items | ForEach-Object {
      $sid, $hex = $_.Split('|')
      $attr = [Convert]::ToUInt32($hex, 16)
      [ordered]@{ sid = $sid; account = (Get-SidName $sid); attributes = "0x$hex"; flags = (ConvertFrom-GroupAttributes $attr) }
    })
}

function ConvertFrom-RawAcl($Acl) {
  if ($null -eq $Acl) { return $null }
  return @($Acl | ForEach-Object {
      $ace = $_
      $entry = [ordered]@{ aceType = [string]$ace.AceType; aceFlags = [string]$ace.AceFlags; mask = $null; sid = $null; account = $null }
      if ($ace -is [Security.AccessControl.KnownAce]) {
        $entry.mask = '0x{0:X8}' -f $ace.AccessMask
        $entry.sid = $ace.SecurityIdentifier.Value
        $entry.account = Get-SidName $entry.sid
      }
      $entry
    })
}

function Get-ProcessSecurityDump {
  # 以 runner 管理员身份（编排脚本自身，提权、尝试启用 SeDebugPrivilege）读取 $ProcessId 的安全信息
  param([int]$ProcessId, [string]$Role)
  $dump = [ordered]@{
    role = $Role; pid = $ProcessId; image = $null; parentPid = $null
    dumpedBy = [ordered]@{ account = $null; sid = $null; elevated = $null; seDebugEnable = $null }
    process = [ordered]@{ sddl = $null; dacl = $null; error = $null }
    token = [ordered]@{
      sddl = $null; dacl = $null; user = $null; groups = $null; integrity = $null; sessionId = $null
      authenticationId = $null; originLogonId = $null; elevationType = $null; elevated = $null
      defaultDacl = $null; logonSession = $null; error = $null
    }
  }
  try {
    $cim = Get-CimInstance -ClassName Win32_Process -Filter "ProcessId=$ProcessId" -ErrorAction Stop
    if ($cim) { $dump.image = $cim.Name; $dump.parentPid = [int]$cim.ParentProcessId }
  } catch { }
  $hProc = [IntPtr]::Zero
  $hTok = [IntPtr]::Zero
  try {
    $me = [Security.Principal.WindowsIdentity]::GetCurrent()
    $dump.dumpedBy.account = $me.Name
    $dump.dumpedBy.sid = $me.User.Value
    $dump.dumpedBy.elevated = ([Security.Principal.WindowsPrincipal]$me).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    Initialize-ProcSec
    $dump.dumpedBy.seDebugEnable = [CaSpikeProcSec]::EnableDebugPrivilege()
    # 优先 ALL_ACCESS（依赖 SeDebugPrivilege）；失败再退到 PQLI|READ_CONTROL
    try { $hProc = [CaSpikeProcSec]::OpenProc($ProcessId, 0x001FFFFF) }
    catch { $hProc = [CaSpikeProcSec]::OpenProc($ProcessId, 0x00021000) }
    $dump.process.sddl = [CaSpikeProcSec]::GetSddl($hProc)
    $dump.process.dacl = ConvertFrom-RawAcl ([Security.AccessControl.RawSecurityDescriptor]::new($dump.process.sddl)).DiscretionaryAcl
  } catch {
    $dump.process.error = $_.Exception.Message
  }
  if ($hProc -ne [IntPtr]::Zero) {
    try {
      # READ_CONTROL | TOKEN_QUERY
      $hTok = [CaSpikeProcSec]::OpenToken($hProc, 0x00020008)
      $t = $dump.token
      $t.sddl = [CaSpikeProcSec]::GetSddl($hTok)
      $t.dacl = ConvertFrom-RawAcl ([Security.AccessControl.RawSecurityDescriptor]::new($t.sddl)).DiscretionaryAcl
      $t.user = @(ConvertFrom-SidAttrList ([CaSpikeProcSec]::SidAndAttributes($hTok, 1)))[0]
      $t.groups = ConvertFrom-SidAttrList ([CaSpikeProcSec]::SidAndAttributes($hTok, 2))
      $t.integrity = @(ConvertFrom-SidAttrList ([CaSpikeProcSec]::SidAndAttributes($hTok, 25)))[0]
      $t.sessionId = [CaSpikeProcSec]::ReadUInt32($hTok, 12, 0)
      $authId = [CaSpikeProcSec]::ReadInt64($hTok, 10, 8)       # TOKEN_STATISTICS.AuthenticationId
      $t.authenticationId = '0x{0:X}' -f $authId
      $t.originLogonId = '0x{0:X}' -f [CaSpikeProcSec]::ReadInt64($hTok, 17, 0)
      $t.elevationType = [CaSpikeProcSec]::ReadUInt32($hTok, 18, 0)
      $t.elevated = ([CaSpikeProcSec]::ReadUInt32($hTok, 20, 0) -ne 0)
      $bytes = [CaSpikeProcSec]::DefaultDacl($hTok)
      if ($bytes.Length -gt 0) { $t.defaultDacl = ConvertFrom-RawAcl ([Security.AccessControl.RawAcl]::new($bytes, 0)) }
      try {
        $ls = ([CaSpikeProcSec]::LogonSession($authId)).Split('|')
        $lt = [int]$ls[0]
        $t.logonSession = [ordered]@{ logonType = $lt; logonTypeName = $LogonTypeNames[$lt]; session = [int]$ls[1]; authPackage = $ls[2]; user = $ls[3] }
      } catch { $t.logonSession = [ordered]@{ error = $_.Exception.Message } }
    } catch {
      $dump.token.error = $_.Exception.Message
    } finally {
      if ($hTok -ne [IntPtr]::Zero) { [void][CaSpikeProcSec]::CloseHandle($hTok) }
      [void][CaSpikeProcSec]::CloseHandle($hProc)
    }
  }
  return $dump
}

function Get-GrantingAces($Dacl, $Token, [uint32]$Rights) {
  # $Dacl 中可能把 $Rights 之一授予 $Token（用户本人或启用中的组）的 allow ACE；deny ACE 一并列出。
  # 只做集合比对（不是 AccessCheck），供人工解释
  if ($null -eq $Dacl -or $null -eq $Token -or $null -eq $Token.user) { return $null }
  $sids = @($Token.user.sid) + @($Token.groups | Where-Object { $_.flags -contains 'Enabled' -and $_.flags -notcontains 'UseForDenyOnly' } | ForEach-Object { $_.sid })
  return @($Dacl | Where-Object {
      $_.sid -and ($sids -contains $_.sid) -and (([Convert]::ToUInt32($_.mask.Substring(2), 16) -band $Rights) -ne 0)
    })
}

function Start-Holder([string]$Label, [string]$As, [string]$Method) {
  # 常驻的 powershell.exe（以 $As 经 $Method 启动），只为让 runner 读取该启动方式下的令牌
  $ps = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
  $script:RunningMihomo.Add($Label)
  Invoke-Users @{
    Action = 'Start'; As = $As; Label = $Label; Method = $Method; FilePath = $ps
    ArgumentList = @('-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 300')
  } | Out-Null
  $job = Read-JobRecord $Label
  return [int]$job.Pid
}

function Stop-Holder([string]$CaseId, [string]$Label) {
  try { Invoke-Users @{ Action = 'Stop'; Label = $Label } | Out-Null } catch { Add-SpikeError "$CaseId 结束 $Label 失败：$($_.Exception.Message)" }
  [void]$script:RunningMihomo.Remove($Label)
}

function Test-OpenAccess {
  # 以当前进程身份对 $ProcessId 分别以 0x1000 / 0x0400 / 0x0010 / 0x1FFFFF 调用 OpenProcess，记录 GetLastError
  param([int]$ProcessId)
  Initialize-ProcSec
  $accesses = @(
    @{ name = 'PROCESS_QUERY_LIMITED_INFORMATION'; mask = [uint32]0x1000 }
    @{ name = 'PROCESS_QUERY_INFORMATION';         mask = [uint32]0x0400 }
    @{ name = 'PROCESS_VM_READ';                   mask = [uint32]0x0010 }
    @{ name = 'PROCESS_ALL_ACCESS';                mask = [uint32]0x1FFFFF }
  )
  return @($accesses | ForEach-Object {
      $h = [IntPtr]::Zero
      $err = 0
      $ok = $false
      try {
        $h = [CaSpikeProcSec]::OpenProc($ProcessId, $_.mask)
        $ok = $true
      } catch {
        if ($_.Exception.InnerException -is [ComponentModel.Win32Exception]) {
          $err = $_.Exception.InnerException.NativeErrorCode
        } elseif ($_.Exception -is [ComponentModel.Win32Exception]) {
          $err = $_.Exception.NativeErrorCode
        } else {
          $err = -1
        }
      } finally {
        if ($h -ne [IntPtr]::Zero) { [void][CaSpikeProcSec]::CloseHandle($h) }
      }
      [ordered]@{ name = $_.name; mask = ('0x{0:X}' -f $_.mask); ok = $ok; win32_error = $err }
    })
}

function Get-LogonSidFromWhoamiCsv([string]$Text) {
  # whoami /groups /fo csv：Attributes 含 "Logon ID" 的 SID。表头固定为 Group Name,Type,SID,Attributes。
  if (-not $Text) { return $null }
  $header = @('Group Name', 'Type', 'SID', 'Attributes')
  $inGroups = $false
  foreach ($line in @($Text -split "`r?`n" | Where-Object { $_.Trim() })) {
    if ($line -match '(?i)^"?Group Name"?') { $inGroups = $true; continue }
    if ($line -match '(?i)^"?(USER INFORMATION|PRIVILEGES INFORMATION|Privilege Name)') { $inGroups = $false; continue }
    if (-not $inGroups -and $line -notmatch 'S-1-') { continue }
    try {
      $row = $line | ConvertFrom-Csv -Header $header
      if ([string]$row.Attributes -match 'Logon ID') { return [string]$row.SID }
    } catch { }
  }
  return $null
}

function Invoke-OpenAccessAs {
  # 以 $As（经 $Method）运行预置脚本：对 $TargetPid 做 4 次 OpenProcess，结果写到作业目录 open-access.json
  param([string]$CaseId, [string]$Step, [string]$As, [string]$Method, [int]$TargetPid)
  $helper = Join-Path $WorkDir 'open-access.ps1'
  if (-not (Test-Path -LiteralPath $helper)) {
    @'
param([Parameter(Mandatory=$true)][int]$TargetPid)
$ErrorActionPreference = "Continue"
# 标准用户写不了 runner 的 %TEMP%；Add-Type 的 csc 临时文件与输出都放到脚本所在作业目录
$here = if ($PSScriptRoot) { $PSScriptRoot } else { (Get-Location).Path }
$env:TEMP = $here; $env:TMP = $here
$dll = Join-Path $here "CaOpenAccess.dll"
$out = Join-Path $here "open-access.json"
if (-not (Test-Path -LiteralPath $dll)) {
  Add-Type -OutputAssembly $dll -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class CaOpenAccess {
  [DllImport("kernel32.dll", SetLastError = true)] public static extern IntPtr OpenProcess(uint a, bool i, int p);
  [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
}
"@
}
Add-Type -Path $dll
$masks = @(
  @{ n = "PROCESS_QUERY_LIMITED_INFORMATION"; m = [uint32]0x1000 },
  @{ n = "PROCESS_QUERY_INFORMATION";         m = [uint32]0x0400 },
  @{ n = "PROCESS_VM_READ";                   m = [uint32]0x0010 },
  @{ n = "PROCESS_ALL_ACCESS";                m = [uint32]0x1FFFFF }
)
$r = foreach ($x in $masks) {
  $h = [CaOpenAccess]::OpenProcess($x.m, $false, $TargetPid)
  $err = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
  $ok = ($h -ne [IntPtr]::Zero)
  if ($ok) { [void][CaOpenAccess]::CloseHandle($h) }
  [ordered]@{ name = $x.n; mask = ("0x{0:X}" -f $x.m); ok = [bool]$ok; win32_error = $err }
}
[ordered]@{ schema = "open-access/v1"; pid = $TargetPid; cwd = $here; attempts = @($r) } |
  ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $out -Encoding UTF8
'@ | Set-Content -LiteralPath $helper -Encoding ASCII
  }
  $ps = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
  $rec = [ordered]@{
    case = $CaseId; step = $Step; kind = 'open-access'; as = $As; method = $Method; targetPid = $TargetPid
    status = $null; exitCode = $null; attempts = $null; error = $null
  }
  try {
    Invoke-Users @{
      Action = 'Run'; As = $As; Label = $Step; Method = $Method; TimeoutSec = $ProbeTimeoutSec
      FilePath = $ps; StagePath = @($helper)
      ArgumentList = @('-NoProfile', '-NonInteractive', '-File', 'open-access.ps1', '-TargetPid', [string]$TargetPid)
    } | Out-Null
    $rec.exitCode = $LASTEXITCODE
    $rec.status = 'ran'
  } catch {
    $rec.status = 'error'
    $rec.error = $_.Exception.Message
  }
  $text = Read-SharedText (Join-Path (Get-JobDir $Step) 'open-access.json')
  if ($text) {
    try { $rec.attempts = @(($text | ConvertFrom-Json).attempts) }
    catch { $rec.error = "open-access.json 不是合法 JSON：$($_.Exception.Message)" }
  } elseif (-not $rec.error) {
    $rec.error = 'open-access.json 未产生'
  }
  Save-JobOutput $Step (Join-Path $ResultsDir "$CaseId\$Step")
  Add-Step $CaseId $Step $rec
  return $rec
}

function Invoke-CaseF2 {
  $methods = @('Credential', 'ScheduledTask')
  $ports = @{ Credential = $PortF2Credential; ScheduledTask = $PortF2Task }
  $analysis = [ordered]@{
    case = 'S-F2'; step = 'sf2-analysis'; kind = 'analysis'
    logonSidSame = [ordered]@{}; caller = [ordered]@{}; targets = [ordered]@{}; error = $null
  }

  # 1) ca_a / ca_b 在两种启动方式下的 whoami /all /fo csv + runner 侧令牌 dump
  foreach ($who in @('ca_a', 'ca_b')) {
    foreach ($pm in $methods) {
      $tag = "$($who)-$($pm.ToLowerInvariant())"
      $step = "sf2-whoami-$tag"
      try {
        Invoke-Users @{
          Action = 'Run'; As = $who; Label = $step; Method = $pm; TimeoutSec = $ProbeTimeoutSec
          FilePath = (Join-Path $env:SystemRoot 'System32\whoami.exe')
          ArgumentList = @('/groups', '/fo', 'csv')
        } | Out-Null
        $exit = $LASTEXITCODE
      } catch {
        $exit = $null
        Add-SpikeError "S-F2/$step：$($_.Exception.Message)"
      }
      Save-JobOutput $step (Join-Path $ResultsDir "S-F2\$step")
      $csv = Read-SharedText (Join-Path (Get-JobDir $step) 'stdout.txt')
      $logonSid = Get-LogonSidFromWhoamiCsv $csv
      # whoami /all 的 CSV 分多段（USER / GROUP / PRIV）；把整段原文保存，另抽 GROUP 段
      $holder = "sf2-holder-$tag"
      $hrec = [ordered]@{
        case = 'S-F2'; step = $holder; kind = 'token-dump'; as = $who; method = $pm
        whoamiExit = $exit; whoamiLogonSid = $logonSid; dump = $null; error = $null
      }
      try {
        $hpid = Start-Holder $holder $who $pm
        $hrec.dump = Get-ProcessSecurityDump -ProcessId $hpid -Role "$who holder（$pm）"
        if (-not $logonSid -and $hrec.dump -and $hrec.dump.token.groups) {
          $logonSid = @($hrec.dump.token.groups | Where-Object { $_.flags -contains 'LogonId' } | ForEach-Object { $_.sid }) | Select-Object -First 1
        }
        if (-not $logonSid -and $hrec.dump -and $hrec.dump.process.dacl) {
          $logonSid = @($hrec.dump.process.dacl | Where-Object { $_.sid -match '^S-1-5-5-' } | ForEach-Object { $_.sid }) | Select-Object -First 1
        }
        $hrec.whoamiLogonSid = $logonSid
      } catch {
        $hrec.error = $_.Exception.Message
        Add-SpikeError "S-F2/$holder：$($hrec.error)"
      } finally {
        Stop-Holder 'S-F2' $holder
      }
      Save-JobOutput $holder (Join-Path $ResultsDir "S-F2\$holder")
      Add-Step 'S-F2' $step ([ordered]@{ case = 'S-F2'; step = $step; kind = 'whoami-groups'; as = $who; method = $pm; exitCode = $exit; logonSid = $logonSid; status = 'ran' })
      Add-Step 'S-F2' $holder $hrec
      if (-not $analysis.caller.Contains($who)) { $analysis.caller[$who] = [ordered]@{} }
      $analysis.caller[$who][$pm] = [ordered]@{
        logonSid = $logonSid
        dumpLogonSids = if ($hrec.dump -and $hrec.dump.token.groups) { @($hrec.dump.token.groups | Where-Object { $_.flags -contains 'LogonId' } | ForEach-Object { $_.sid }) } else { @() }
        integrity = if ($hrec.dump) { $hrec.dump.token.integrity } else { $null }
        logonSession = if ($hrec.dump) { $hrec.dump.token.logonSession } else { $null }
        defaultDacl = if ($hrec.dump) { $hrec.dump.token.defaultDacl } else { $null }
        tokenSddl = if ($hrec.dump) { $hrec.dump.token.sddl } else { $null }
        processSddl = if ($hrec.dump) { $hrec.dump.process.sddl } else { $null }
      }
    }
  }
  # 显式比较：同一种启动方式下 ca_a 与 ca_b 的 logon SID 是否相同
  foreach ($pm in $methods) {
    $a = $analysis.caller['ca_a'][$pm].logonSid
    $b = $analysis.caller['ca_b'][$pm].logonSid
    $analysis.logonSidSame[$pm] = [ordered]@{ ca_a = $a; ca_b = $b; same = (($null -ne $a) -and ($a -eq $b)) }
  }

  # 2) ca_b 分别经 Credential / ScheduledTask 启动 verge-mihomo.exe；runner dump；ca_a 经两种方式做 tcp-probe + OpenProcess 矩阵
  foreach ($mm in $methods) {
    $tag = $mm.ToLowerInvariant()
    $secret = New-RandomHex 16
    $script:Scrub.Add($secret)
    $port = $ports[$mm]
    $mihomo = @{ Label = "sf2-mihomo-$tag"; As = 'ca_b'; ExtCtl = "127.0.0.1:$port"; Secret = $secret; Method = $mm }
    Invoke-WithMihomo 'S-F2' $mihomo {
      param($m)
      $target = [ordered]@{
        launchMethod = $mm; actualMethod = $m.method; port = $port
        mihomo = $null; parent = $null; openAccess = [ordered]@{}; probes = [ordered]@{}
      }
      if ($m.method -ne $mm) { Add-SpikeError "S-F2/$($m.step)：要求 $mm，实际 $($m.method)" }
      if ($m.pid) {
        $target.mihomo = Get-ProcessSecurityDump -ProcessId $m.pid -Role "ca_b verge-mihomo（$mm）"
        if ($target.mihomo.parentPid) {
          $target.parent = Get-ProcessSecurityDump -ProcessId $target.mihomo.parentPid -Role "ca_b wrapper（$mm）"
        }
        # 目标令牌中的 logon SID
        $targetLogon = if ($target.mihomo.token.groups) {
          @($target.mihomo.token.groups | Where-Object { $_.flags -contains 'LogonId' } | ForEach-Object { $_.sid })
        } else { @() }
        $target['targetLogonSids'] = $targetLogon
        $callerLogon = $analysis.caller['ca_a'][$mm].logonSid
        $target['logonSidSameAsCaller'] = (($null -ne $callerLogon) -and ($targetLogon -contains $callerLogon))
      }
      foreach ($pm in $methods) {
        $ptag = $pm.ToLowerInvariant()
        # ca_a OpenProcess 矩阵
        if ($m.pid) {
          $oa = Invoke-OpenAccessAs -CaseId 'S-F2' -Step "sf2-$tag-open-$ptag" -As 'ca_a' -Method $pm -TargetPid $m.pid
          $target.openAccess[$pm] = $oa.attempts
        }
        # ca_a tcp-probe
        $step = "sf2-$tag-probe-$ptag"
        $r = Invoke-ProbeTcp -CaseId 'S-F2' -Step $step -As 'ca_a' -Port $port -Method $pm `
          -Expect '仅记录：l2_1 / l3 是否成功；与 open-access 矩阵对照' -Evidence {
          param($rep) Get-TcpEvidence $rep
        }
        $pp = $null
        if ($null -ne $r.evidence -and $r.evidence.Contains('per_pid')) {
          $pp = @($r.evidence.per_pid | Where-Object { $_.pid -eq $m.pid }) | Select-Object -First 1
        }
        $callerTok = $analysis.caller['ca_a'][$pm]
        # 用 holder dump 的完整令牌（含启用中的组 + logon SID）解释 DACL 授予
        $callerDumpTok = $null
        $holderKey = "sf2-holder-ca_a-$ptag"
        # analysis.caller 已保存 dump 摘要；重建一个最小 token 对象给 Get-GrantingAces
        if ($callerTok) {
          $groups = @()
          if ($callerTok.logonSid) { $groups += [ordered]@{ sid = $callerTok.logonSid; flags = @('Enabled', 'LogonId') } }
          $callerDumpTok = [ordered]@{ user = [ordered]@{ sid = $script:Users['ca_a'].Sid }; groups = $groups }
        }
        $target.probes[$pm] = [ordered]@{
          step = $step
          listenerPids = if ($null -ne $r.evidence) { $r.evidence.listener_pids } else { $null }
          openProcess = if ($pp) { $pp.l2_1 } else { $null }
          openToken = if ($pp) { $pp.l3 } else { $null }
          processDaclGrantsToLogonSid = if ($callerDumpTok -and $target.mihomo) {
            Get-GrantingAces $target.mihomo.process.dacl $callerDumpTok ([uint32]0x1000)
          } else { $null }
          tokenDaclGrantsToLogonSid = if ($callerDumpTok -and $target.mihomo) {
            Get-GrantingAces $target.mihomo.token.dacl $callerDumpTok ([uint32]0x0008)
          } else { $null }
        }
        [void]$holderKey
      }
      Write-Json (Join-Path $ResultsDir "S-F2\sf2-target-$tag.json") $target
      $analysis.targets[$mm] = $target
    }
  }
  Add-Step 'S-F2' 'sf2-analysis' $analysis
}

# ---------------------------------------------------------------- S-D′：默认 SDDL + 第二实例（Ellie F3）
# ca_a 以默认 SDDL 启动 mihomo；ca_b 用 NamedPipeServerStream(MaxAllowedServerInstances，无 FirstPipeInstance)
# 在同名管道上创建第二个服务端实例；ca_a 用 pipe-probe 连约 6 次，记录每次的 server_pid / owner / l3。

function Start-SecondPipeInstance {
  # 以 ca_b 启动预置脚本：创建第二实例 → 写 ready → 循环 Accept → 直到 stop 文件
  param([string]$Step, [string]$As, [string]$Method, [string]$PipeName, [string]$ReadyFile, [string]$StopFile, [string]$ResultFile)
  $leaf = if ($PipeName -match '^[\\/]{2}\.[\\/]pipe[\\/](.+)$') { $Matches[1] } else { $PipeName }
  $helper = Join-Path $WorkDir 'second-pipe.ps1'
  @'
param(
  [Parameter(Mandatory=$true)][string]$Leaf,
  [Parameter(Mandatory=$true)][string]$ReadyFile,
  [Parameter(Mandatory=$true)][string]$StopFile,
  [Parameter(Mandatory=$true)][string]$ResultFile
)
$ErrorActionPreference = "Stop"
$doc = [ordered]@{ schema = "second-pipe/v1"; leaf = $Leaf; createOk = $false; hresult = $null; win32 = $null; message = $null; accepts = 0 }
try {
  $server = New-Object System.IO.Pipes.NamedPipeServerStream(
    $Leaf,
    [System.IO.Pipes.PipeDirection]::InOut,
    [System.IO.Pipes.NamedPipeServerStream]::MaxAllowedServerInstances,
    [System.IO.Pipes.PipeTransmissionMode]::Byte,
    [System.IO.Pipes.PipeOptions]::Asynchronous,
    0, 0)
  $doc.createOk = $true
  ($doc | ConvertTo-Json) | Set-Content -LiteralPath $ResultFile -Encoding UTF8
  "ready" | Set-Content -LiteralPath $ReadyFile -Encoding ASCII
  while (-not (Test-Path -LiteralPath $StopFile)) {
    $ar = $server.BeginWaitForConnection($null, $null)
    while (-not $ar.IsCompleted -and -not (Test-Path -LiteralPath $StopFile)) { Start-Sleep -Milliseconds 200 }
    if (Test-Path -LiteralPath $StopFile) { break }
    try {
      $server.EndWaitForConnection($ar)
      $doc.accepts++
      # 标识响应：让 pipe-probe 的 http_version 能区分第二实例（Ellie F3）
      try {
        $writer = New-Object System.IO.StreamWriter($server)
        $body = '{"pipe_probe":"second"}'
        $payload = "HTTP/1.1 503 Service Unavailable`r`nContent-Length: $($body.Length)`r`nConnection: close`r`n`r`n$body"
        $writer.Write($payload)
        $writer.Flush()
      } catch { $doc.message = "write: $($_.Exception.Message)" }
      Start-Sleep -Milliseconds 200
      ($doc | ConvertTo-Json) | Set-Content -LiteralPath $ResultFile -Encoding UTF8
      try { $server.Disconnect() } catch { }
    } catch {
      $doc.message = $_.Exception.Message
      ($doc | ConvertTo-Json) | Set-Content -LiteralPath $ResultFile -Encoding UTF8
      break
    }
  }
  $server.Dispose()
} catch {
  $ex = $_.Exception
  $doc.message = $ex.Message
  $doc.hresult = $ex.HResult
  if ($ex.InnerException) { $doc.message = $doc.message + " | " + $ex.InnerException.Message; if ($ex.InnerException.HResult) { $doc.hresult = $ex.InnerException.HResult } }
  if ($doc.hresult -and (($doc.hresult -band 0xFFFF0000) -eq [int]0x80070000)) { $doc.win32 = ($doc.hresult -band 0xFFFF) }
  ($doc | ConvertTo-Json) | Set-Content -LiteralPath $ResultFile -Encoding UTF8
  if (-not (Test-Path -LiteralPath $ReadyFile)) { "failed" | Set-Content -LiteralPath $ReadyFile -Encoding ASCII }
  exit 1
}
exit 0
'@ | Set-Content -LiteralPath $helper -Encoding ASCII
  $ps = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
  $script:RunningMihomo.Add($Step)
  Invoke-Users @{
    Action = 'Start'; As = $As; Label = $Step; Method = $Method
    FilePath = $ps; StagePath = @($helper)
    ArgumentList = @(
      '-NoProfile', '-NonInteractive', '-File', 'second-pipe.ps1',
      '-Leaf', $leaf, '-ReadyFile', $ReadyFile, '-StopFile', $StopFile, '-ResultFile', $ResultFile
    )
  } | Out-Null
  return (Read-JobRecord $Step)
}

function Invoke-CaseDPrime {
  $a = $script:Users['ca_a']
  $pipe = "${PipePrefix}verge-mihomo-sidecar-release-$($a.OwnerKey)"
  $nProbes = 6
  $analysis = [ordered]@{
    case = 'S-D-prime'; step = 'sdp-analysis'; kind = 'analysis'
    pipe = $pipe; secondInstance = $null; probes = @(); distinctServerPids = @(); error = $null
  }

  # ca_a 以默认 SDDL（不设 LISTEN_NAMEDPIPE_SDDL）启动 mihomo
  Invoke-WithMihomo 'S-D-prime' @{ Label = 'sdp-mihomo'; As = 'ca_a'; PipeName = $pipe } {
    param($m)
    if ($m.pipeStatus -ne 'listening') {
      Add-SpikeError "S-D-prime：ca_a mihomo 未监听（$($m.pipeStatus)）"
      return
    }
    # 第二实例放进 ca_b 自己的作业目录（可写）；ready / stop / result 都在那里
    $secStep = 'sdp-second'
    $jobDir = Get-JobDir $secStep
    # 作业目录要先由 Start 创建；先占位再读路径——Start-SecondPipeInstance 内部会建
    # 用 WorkDir 下的共享协调目录，ACL 给 ca_a + ca_b Modify
    $coord = Join-Path $StageDir 'sdp-coord'
    New-Item -ItemType Directory -Force -Path $coord | Out-Null
    if ($IsWindows -or $env:OS -match 'Windows') {
      try {
        $acl = Get-Acl -LiteralPath $coord
        foreach ($sid in @($script:Users['ca_a'].Sid, $script:Users['ca_b'].Sid)) {
          $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            ([Security.Principal.SecurityIdentifier]$sid), 'Modify', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
          $acl.AddAccessRule($rule)
        }
        Set-Acl -LiteralPath $coord -AclObject $acl
      } catch {
        Write-Info "S-D-prime：设置 coord ACL 失败（$($_.Exception.Message)），继续试"
      }
    }
    $ready = Join-Path $coord 'ready.txt'
    $stop = Join-Path $coord 'stop.txt'
    $result = Join-Path $coord 'second.json'
    Remove-Item -LiteralPath $ready, $stop, $result -Force -ErrorAction SilentlyContinue

    $job = $null
    try {
      $job = Start-SecondPipeInstance -Step $secStep -As 'ca_b' -Method 'ScheduledTask' `
        -PipeName $pipe -ReadyFile $ready -StopFile $stop -ResultFile $result
    } catch {
      Add-SpikeError "S-D-prime/$secStep 启动失败：$($_.Exception.Message)"
    }

    $deadline = (Get-Date).AddSeconds($ListenWaitSec)
    while ((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath $ready)) { Start-Sleep -Milliseconds 250 }
    $secRec = [ordered]@{
      case = 'S-D-prime'; step = $secStep; kind = 'second-pipe'; as = 'ca_b'; method = 'ScheduledTask'
      ready = (Test-Path -LiteralPath $ready); create = $null; error = $null; status = 'ran'
    }
    if (Test-Path -LiteralPath $result) {
      try { $secRec.create = (Get-Content -Raw -Encoding UTF8 -LiteralPath $result | ConvertFrom-Json) } catch { $secRec.error = $_.Exception.Message }
    } elseif (-not $secRec.ready) {
      $secRec.error = "$ListenWaitSec 秒内没有 ready 文件"
      $secRec.status = 'error'
      Add-SpikeError "S-D-prime/$secStep：$($secRec.error)"
    }
    $analysis.secondInstance = $secRec.create
    Save-JobOutput $secStep (Join-Path $ResultsDir "S-D-prime\$secStep")
    if (Test-Path -LiteralPath $result) {
      Copy-Item -LiteralPath $result -Destination (Join-Path $ResultsDir "S-D-prime\$secStep\second.json") -Force
    }
    Add-Step 'S-D-prime' $secStep $secRec

    # 不论第二实例是否创建成功，都让 ca_a 连 N 次，记录落到哪个 server_pid
    $pids = New-Object System.Collections.Generic.List[int]
    for ($i = 1; $i -le $nProbes; $i++) {
      $step = "sdp-probe-{0:D2}" -f $i
      $r = Invoke-ProbePipe -CaseId 'S-D-prime' -Step $step -As 'ca_a' -PipeName $pipe `
        -Expect '记录 server_pid / owner / l3；若连到 ca_b 的第二实例则 server_pid ≠ mihomo' -Evidence {
        param($rep)
        [ordered]@{
          open = Get-StepBrief (Get-ProbeStep $rep 'open')
          server_pid = Get-Field (Get-ProbeStep $rep 'server_pid') 'value'
          owner_sid = Get-PipeOwnerSid $rep
          l3 = Get-StepBrief (Get-ProbeStep $rep 'l3')
          l3_user_sid = Get-Field (Get-ProbeStep $rep 'l3') 'value.token_user.sid'
          l2_1_file = Get-Field (Get-ProbeStep $rep 'l2_1') 'value.file_name'
          http_status = Get-Field (Get-ProbeStep $rep 'http_version') 'value.status'
          http_body_pipe_probe = Get-Field (Get-ProbeStep $rep 'http_version') 'value.body_json.pipe_probe'
        }
      }
      $ev = $r.evidence
      if ($ev -and $null -ne $ev.server_pid) { [void]$pids.Add([int]$ev.server_pid) }
      $secondPid = if ($job) { [int]$job.Pid } else { $null }
      $hitSecond = $false
      if ($ev) {
        if ($secondPid -and $ev.server_pid -and ([int]$ev.server_pid -eq $secondPid)) { $hitSecond = $true }
        if ($ev.http_body_pipe_probe -eq 'second') { $hitSecond = $true }
        if ($m.pid -and $ev.server_pid -and ([int]$ev.server_pid -ne [int]$m.pid)) { $hitSecond = $true }
      }
      $analysis.probes += [ordered]@{
        step = $step
        server_pid = if ($ev) { $ev.server_pid } else { $null }
        owner_sid = if ($ev) { $ev.owner_sid } else { $null }
        l3_user_sid = if ($ev) { $ev.l3_user_sid } else { $null }
        l2_1_file = if ($ev) { $ev.l2_1_file } else { $null }
        http_status = if ($ev) { $ev.http_status } else { $null }
        http_body_pipe_probe = if ($ev) { $ev.http_body_pipe_probe } else { $null }
        secondPid = $secondPid
        reachedSecond = $hitSecond
      }
    }
    $analysis.distinctServerPids = @($pids | Select-Object -Unique)
    $analysis['mihomoPid'] = $m.pid

    # 通知第二实例退出
    'stop' | Set-Content -LiteralPath $stop -Encoding ASCII
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath (Join-Path (Get-JobDir $secStep) 'exitcode.txt'))) {
      Start-Sleep -Milliseconds 300
    }
    if (-not (Test-Path -LiteralPath (Join-Path (Get-JobDir $secStep) 'exitcode.txt'))) {
      try { Invoke-Users @{ Action = 'Stop'; Label = $secStep } | Out-Null } catch { }
    }
    [void]$script:RunningMihomo.Remove($secStep)
    if (Test-Path -LiteralPath $result) {
      try { $analysis.secondInstance = (Get-Content -Raw -Encoding UTF8 -LiteralPath $result | ConvertFrom-Json) } catch { }
      Copy-Item -LiteralPath $result -Destination (Join-Path $ResultsDir "S-D-prime\$secStep\second.json") -Force
    }
    Save-JobOutput $secStep (Join-Path $ResultsDir "S-D-prime\$secStep")
  }
  Add-Step 'S-D-prime' 'sdp-analysis' $analysis
}


# ---------------------------------------------------------------- 清理

function Invoke-SpikeCleanup {
  foreach ($label in @($script:RunningMihomo)) {
    try { Invoke-Users @{ Action = 'Stop'; Label = $label } | Out-Null } catch { Add-SpikeError "清理：结束 $label 失败：$($_.Exception.Message)" }
  }
  # Cleanup：结束作业与用户名下进程、删除 ca-spike-* 计划任务、撤销 SeBatchLogonRight、删除用户、配置文件与状态目录
  # 先按映像名强杀，避免配置文件被占用导致 Cleanup 非零
  foreach ($name in @('verge-mihomo', 'pipe-probe', 'powershell')) {
    foreach ($p in @(Get-Process -Name $name -ErrorAction SilentlyContinue)) {
      try { Stop-Process -Id $p.Id -Force -ErrorAction Stop } catch { }
    }
  }
  Start-Sleep -Milliseconds 500
  try {
    Invoke-Users @{ Action = 'Cleanup'; UserName = @('ca_a', 'ca_b') } | Out-Null
    if ($LASTEXITCODE -ne 0) {
      # 配置文件“正在使用”在 runner 上偶发；用户与计划任务已删则只警告
      Write-Info "ci-standard-users Cleanup 退出码 $LASTEXITCODE（若仅配置文件占用，可忽略）"
    }
  } catch {
    Add-SpikeError "ci-standard-users Cleanup 失败：$($_.Exception.Message)"
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
  cases      = $Cases
  runner     = [ordered]@{ os = [Environment]::OSVersion.VersionString; ps = $PSVersionTable.PSVersion.ToString() }
  mihomo     = $null
  probe      = [ordered]@{ path = $ProbePath; present = $false; sha256 = $null }
  users      = $null
  methods    = $null
  steps      = $null
  errors     = $null
  note       = '只记录不判定：P1 ~ P7 需人工按 §4.1 判定；evidence 只是按 spike/pipe-probe/README.md 摘出的字段，以各步 output 为准'
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

  # S-A ~ S-E 优先（S-R8），S-F / S-F2 最后
  $plan = @(
    @{ ids = @('S-A', 'S-B'); fn = ${function:Invoke-CaseAB} }
    @{ ids = @('S-C'); fn = ${function:Invoke-CaseC} }
    @{ ids = @('S-D'); fn = ${function:Invoke-CaseD} }
    @{ ids = @('S-E'); fn = ${function:Invoke-CaseE} }
    @{ ids = @('S-F'); fn = ${function:Invoke-CaseF} }
    @{ ids = @('S-F2'); fn = ${function:Invoke-CaseF2} }
    @{ ids = @('S-D-prime'); fn = ${function:Invoke-CaseDPrime} }
  )
  foreach ($c in $plan) {
    if (-not @($c.ids | Where-Object { $Cases -contains $_ })) { continue }
    try { & $c.fn } catch { Add-SpikeError "用例 $($c.ids -join '/') 执行异常：$($_.Exception.Message)" }
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
