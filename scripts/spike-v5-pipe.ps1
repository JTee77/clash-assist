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
  foreach ($f in @('identity.txt', 'stdout.txt', 'stderr.txt', 'exitcode.txt', 'wrapper-error.txt', 'job.json', 'probe.json', 'ready.json')) {
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
    status = $null; method = $null; exitCode = $null; timedOut = $false
    output = $null; outputSource = $null; context = $null; evidence = $null; identity = $null; error = $null
    verdict = $null   # 只记录；P1 ~ P7 由人工判定
  }
}

function Complete-ProbeRecord($Rec, [scriptblock]$Evidence) {
  # 判定只看 JSON，不看退出码（README）；退出码照样记录。优先读 --out，拿不到再看 stdout。
  $step = $Rec.step
  $rec.method = Remember-Method $Rec.as $step
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
  param([string]$CaseId, [string]$Step, [string]$As, [string[]]$ProbeArgs, [string]$Expect, [scriptblock]$Evidence)
  $fullArgs = @($ProbeArgs) + @('--out', (Get-ProbeOutPath $Step))
  $rec = New-ProbeRecord $CaseId $Step $As $fullArgs $Expect
  if (-not $ProbePath -or -not (Test-Path -LiteralPath $ProbePath)) {
    $rec.status = 'probe-missing'
    Add-Step $CaseId $Step $rec
    return $rec
  }
  $params = @{
    Action = 'Run'; As = $As; Label = $Step
    StagePath = @($ProbePath); FilePath = (Split-Path -Leaf $ProbePath)
    ArgumentList = $fullArgs; TimeoutSec = $ProbeTimeoutSec
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
  param([string]$CaseId, [string]$Step, [string]$As, [int]$Port, [string]$Expect, [scriptblock]$Evidence)
  return (Invoke-Probe -CaseId $CaseId -Step $Step -As $As -Expect $Expect -Evidence $Evidence `
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
