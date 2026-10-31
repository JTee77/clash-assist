<#
.SYNOPSIS
  在 GitHub Windows runner 上创建 / 清理测试用标准用户，并以其（或 SYSTEM）身份运行进程。
  windows-plan r4 §3、§4.1（V5 CI spike 第 3、5、6 步）、§6 M1（S-4 ②③④）。M0 雏形。

.DESCRIPTION
  runner 以提权的 runneradmin（UAC 关闭）运行：它创建的对象属主是 BA，它作为客户端也天然命中
  BA 的 ACE，不能代表普通用户。本脚本临时创建只属于 Users 组的标准用户（默认 ca_a、ca_b），
  再用其身份运行 mihomo / 探针，供 S-A ~ S-F 与 M1 标准用户用例使用。

  本脚本只做“创建用户、以某身份运行、记录、清理”，不做任何放行 / 拒绝判断。

  动作（-Action）：
    Create   用 New-LocalUser 创建用户（口令在内存中随机生成为 SecureString，不出现在任何命令行，
             也不以明文落盘），确认只在 Users 组、不在 Administrators 组；
             取得 SID 与 Verge 属主键 ownerKey = hex(sha256(SID 字符串的 UTF-8 字节))（与 2.1
             verge_owner_key 一致）。users.json 只记 Name、Sid、OwnerKey（无机密）。
             后续 Start 需要凭据，因此把 PSCredential 用 Export-Clixml 写到状态目录的
             secrets\<名>.cred.xml：口令部分由 DPAPI 按当前用户（runner 管理员账户）加密，
             其他账户无法解密；目录 ACL 只允许 SYSTEM / Administrators。
             在 GitHub Actions 中写 $GITHUB_OUTPUT：<名>_sid、<名>_key。
    Show     打印已创建用户（不含口令）。
    Start    以 -As 指定的身份（已创建的用户名，或 SYSTEM）后台启动 -FilePath，立即返回作业对象
             （Label、JobDir、Pid、Method）。适合 mihomo 这类常驻进程。
    Run      同 Start，但等待进程退出（最多 -TimeoutSec 秒），打印 stdout/stderr，
             并以子进程的退出码退出（超时则结束进程并返回 124）。适合探针。
    Stop     结束 -Label 指定作业的进程（含子进程）。
    Cleanup  结束本脚本启动的全部进程及上述用户名下的残留进程，删除计划任务（ca-spike-*），
             撤销授予的登录权限，`net user <名> /delete`，删除用户配置文件与状态目录。可重复执行。

  以其他用户身份运行的方式（-Method，§4.1 第 5 步）：
    Credential     Start-Process -Credential（依赖 Secondary Logon 服务 seclogon，脚本会尝试启动它；
                   runner 会话是否允许待验证）。
    ScheduledTask  Register-ScheduledTask 以“用户 + 口令”注册任务并立即运行。标准用户默认没有
                   “作为批处理作业登录”（SeBatchLogonRight）权限，脚本会经 LSA 临时授予，Cleanup 时撤销。
                   以 SYSTEM 运行时总是用此方式（New-ScheduledTaskPrincipal -UserId SYSTEM）。
    Auto（默认）   先试 Credential；抛错或 30 秒内进程未起来，则改用 ScheduledTask。
                   实际采用的方式记录在作业对象的 Method 字段中。

  每个作业有独立目录 <StateDir>\jobs\<Label>\（仅 SYSTEM、Administrators 与运行身份可访问），内含：
    spec.json      要运行的程序、参数与额外环境变量
    wrapper.ps1    以目标身份运行的包装脚本（设置环境变量 → 启动程序 → 记录）
    identity.txt   `whoami /user /groups` 输出（用于确认身份与“不含管理员组”，例如 P7 环境检查）
    pid.txt        目标程序的 PID
    stdout.txt / stderr.txt / exitcode.txt
  环境变量（如 LISTEN_NAMEDPIPE_SDDL）由包装脚本在目标身份的进程内设置，不依赖父进程环境继承。

.PARAMETER Action
  Create | Show | Start | Run | Stop | Cleanup。

.PARAMETER UserName
  要创建 / 清理的用户名列表，默认 ca_a、ca_b（不超过 20 个字符）。

.PARAMETER StateDir
  状态目录，默认 $env:RUNNER_TEMP\ca-standard-users（无 RUNNER_TEMP 时用 $env:TEMP）。

.PARAMETER As
  Start / Run：运行身份，为 Create 创建过的用户名或 SYSTEM。

.PARAMETER FilePath
  Start / Run：要运行的程序。相对路径按作业目录解析（配合 -StagePath）。
  注意映像文件名会被 L2 检查，mihomo 须事先改名为 verge-mihomo.exe。

.PARAMETER ArgumentList
  Start / Run：程序参数（数组），脚本按 Windows 命令行规则逐个加引号。

.PARAMETER Environment
  Start / Run：额外环境变量，哈希表，例如 @{ LISTEN_NAMEDPIPE_SDDL = 'D:P(A;;GA;;;<SID>)(A;;GA;;;SY)(A;;GA;;;BA)' }。

.PARAMETER StagePath
  Start / Run：启动前复制到作业目录的文件或目录（例如 verge-mihomo.exe 与 config 目录），
  避免运行身份读不到 runner 工作区。

.PARAMETER Method
  Auto | Credential | ScheduledTask，见上文。

.PARAMETER Label
  Start / Run / Stop：作业名（字母、数字、- 和 _）。Start / Run 省略时自动生成。

.PARAMETER TimeoutSec
  Run：等待退出的秒数，默认 120。

.EXAMPLE
  # 在 workflow 的 `shell: pwsh` 步骤中用 & 调用（-Environment 需要哈希表，不能用 pwsh -File 传）：
  # 取 SID / ownerKey 用步骤输出（steps.<id>.outputs.ca_a_sid / ca_a_key）或 -Action Show，
  # 不要直接读取或打印状态目录里的文件。
  & ./scripts/ci-standard-users.ps1 -Action Create
  & ./scripts/ci-standard-users.ps1 -Action Show
  $a = [pscustomobject]@{ Sid = $env:CA_A_SID; OwnerKey = $env:CA_A_KEY }   # 由上一步输出传入

.EXAMPLE
  # S-A：ca_a 运行 mihomo（Verge sidecar 管道名与 SDDL），ca_a 运行探针
  $pipe = "\\.\pipe\verge-mihomo-sidecar-release-$($a.OwnerKey)"
  $sddl = "D:P(A;;GA;;;$($a.Sid))(A;;GA;;;SY)(A;;GA;;;BA)"
  & ./scripts/ci-standard-users.ps1 -Action Start -As ca_a -Label sa-mihomo `
      -StagePath spike\bin\verge-mihomo.exe, spike\conf -FilePath verge-mihomo.exe `
      -ArgumentList '-d', 'conf', '-ext-ctl-pipe', $pipe `
      -Environment @{ LISTEN_NAMEDPIPE_SDDL = $sddl }
  & ./scripts/ci-standard-users.ps1 -Action Run -As ca_a -Label sa-probe `
      -StagePath spike\bin\pipe-probe.exe -FilePath pipe-probe.exe -ArgumentList 'pipe', $pipe
  if ($LASTEXITCODE -ne 0) { Write-Warning "probe exit $LASTEXITCODE" }   # 探针只记录，不判定

.EXAMPLE
  # S-C：SYSTEM 运行 mihomo（计划任务）
  & ./scripts/ci-standard-users.ps1 -Action Start -As SYSTEM -Label sc-mihomo -FilePath verge-mihomo.exe ...

.EXAMPLE
  # 收尾（放在 `if: always()` 的步骤里）
  & ./scripts/ci-standard-users.ps1 -Action Cleanup

.NOTES
  需要管理员权限（runner 默认满足）。兼容 PowerShell 7 与 Windows PowerShell 5.1。
  禁止把状态目录（-StateDir，默认 $RUNNER_TEMP\ca-standard-users）作为 artifact 上传：
  其中 secrets\ 下是 DPAPI 保护的凭据文件；artifact 只上传探针输出的 JSON。
  “待验证”：Credential 方式在 runner 会话中是否可用、SeBatchLogonRight 授予后计划任务能否以
  标准用户运行，均在 V5 spike 当天实测，结论记入 M1 PR。
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true, Position = 0)]
  [ValidateSet('Create', 'Show', 'Start', 'Run', 'Stop', 'Cleanup')]
  [string]$Action,

  [ValidatePattern('^[A-Za-z0-9_]{1,20}$')]
  [string[]]$UserName = @('ca_a', 'ca_b'),

  [string]$StateDir,

  [string]$As,

  [string]$FilePath,

  [string[]]$ArgumentList = @(),

  [hashtable]$Environment = @{},

  [string[]]$StagePath = @(),

  [ValidateSet('Auto', 'Credential', 'ScheduledTask')]
  [string]$Method = 'Auto',

  [ValidatePattern('^[A-Za-z0-9_-]{1,64}$')]
  [string]$Label,

  [ValidateRange(1, 86400)]
  [int]$TimeoutSec = 120
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

$SidSystem = 'S-1-5-18'
$SidAdmins = 'S-1-5-32-544'
$SidUsers = 'S-1-5-32-545'
$TaskPrefix = 'ca-spike-'
$BatchRight = 'SeBatchLogonRight'
$StartWaitSec = 30
# 新用户首次经计划任务登录要先建配置文件，windows-2022 上实测超过 30 秒（MUST-4 回放首跑）；
# 等到 pid.txt 出现即返回，所以放宽上限不拖慢正常启动
$TaskStartWaitSec = 120
$TaskTimeLimitMin = 30

if (-not $StateDir) {
  $base = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
  $StateDir = Join-Path $base 'ca-standard-users'
}
$UsersFile = Join-Path $StateDir 'users.json'
$SecretsDir = Join-Path $StateDir 'secrets'
$JobsDir = Join-Path $StateDir 'jobs'
$GrantsFile = Join-Path $StateDir 'grants.json'

function Write-Info([string]$Message) { Write-Host "[ci-standard-users] $Message" }

function Assert-Windows {
  $isWin = ($PSVersionTable.PSEdition -eq 'Desktop') -or ((Get-Variable -Name IsWindows -ValueOnly -ErrorAction SilentlyContinue) -eq $true)
  if (-not $isWin) { throw '只能在 Windows 上运行' }
}

function Assert-Admin {
  $id = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($id)
  if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw '需要管理员权限（GitHub Windows runner 默认以提权的 runneradmin 运行）'
  }
}

function Get-OwnerKey([string]$Sid) {
  $sha = [Security.Cryptography.SHA256]::Create()
  try {
    $hash = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Sid))
  } finally {
    $sha.Dispose()
  }
  return (($hash | ForEach-Object { $_.ToString('x2') }) -join '')
}

function New-RandomPassword {
  # 返回 SecureString，口令不以 .NET 字符串形式存在。
  # 24 位：保证大写、小写、数字、符号各至少一个（满足 Windows Server 默认复杂度策略）；
  # 符号集避开引号、空格、/、^、&、|、<、>、%，便于排障时手工输入。
  $upper = 'ABCDEFGHJKLMNPQRSTUVWXYZ'
  $lower = 'abcdefghijkmnopqrstuvwxyz'
  $digit = '23456789'
  $symbol = '!#$*+-=?@_~.'
  $all = $upper + $lower + $digit + $symbol
  $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
  try {
    $pick = {
      param([string]$Set)
      $b = New-Object byte[] 4
      $rng.GetBytes($b)
      $Set[[int]([BitConverter]::ToUInt32($b, 0) % [uint32]$Set.Length)]
    }
    $chars = New-Object System.Collections.Generic.List[char]
    foreach ($set in @($upper, $lower, $digit, $symbol)) { $chars.Add((& $pick $set)) }
    while ($chars.Count -lt 24) { $chars.Add((& $pick $all)) }
    # Fisher–Yates 洗牌
    for ($i = $chars.Count - 1; $i -gt 0; $i--) {
      $b = New-Object byte[] 4
      $rng.GetBytes($b)
      $j = [int]([BitConverter]::ToUInt32($b, 0) % [uint32]($i + 1))
      $tmp = $chars[$i]; $chars[$i] = $chars[$j]; $chars[$j] = $tmp
    }
    $sec = New-Object Security.SecureString
    foreach ($c in $chars) { $sec.AppendChar($c) }
    $chars.Clear()
    $sec.MakeReadOnly()
    return $sec
  } finally {
    $rng.Dispose()
  }
}

function ConvertTo-WindowsArgument([string]$Value) {
  # 按 CommandLineToArgvW 规则加引号。
  if ($Value.Length -gt 0 -and $Value -notmatch '[\s"]') { return $Value }
  $sb = New-Object Text.StringBuilder
  [void]$sb.Append('"')
  $backslashes = 0
  foreach ($ch in $Value.ToCharArray()) {
    if ($ch -eq [char]'\') { $backslashes++; continue }
    if ($ch -eq [char]'"') {
      [void]$sb.Append('\' * ($backslashes * 2 + 1)).Append('"')
    } else {
      [void]$sb.Append('\' * $backslashes).Append($ch)
    }
    $backslashes = 0
  }
  [void]$sb.Append('\' * ($backslashes * 2)).Append('"')
  return $sb.ToString()
}

function Join-WindowsArguments([string[]]$Values) {
  return (@($Values | ForEach-Object { ConvertTo-WindowsArgument $_ }) -join ' ')
}

function Set-PrivateAcl([string]$Path, [string[]]$ModifySids = @()) {
  # 关闭继承，只授予 SYSTEM、Administrators 完全控制，以及（可选）运行身份修改权限。
  $acl = New-Object Security.AccessControl.DirectorySecurity
  $acl.SetAccessRuleProtection($true, $false)
  $inherit = [Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
  $none = [Security.AccessControl.PropagationFlags]::None
  $allow = [Security.AccessControl.AccessControlType]::Allow
  foreach ($sid in @($SidSystem, $SidAdmins)) {
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule(
          (New-Object Security.Principal.SecurityIdentifier($sid)), 'FullControl', $inherit, $none, $allow)))
  }
  foreach ($sid in $ModifySids) {
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule(
          (New-Object Security.Principal.SecurityIdentifier($sid)), 'Modify', $inherit, $none, $allow)))
  }
  Set-Acl -LiteralPath $Path -AclObject $acl
}

function Initialize-StateDir {
  if (-not (Test-Path -LiteralPath $StateDir)) {
    New-Item -ItemType Directory -Path $StateDir | Out-Null
  }
  Set-PrivateAcl -Path $StateDir
  if (-not (Test-Path -LiteralPath $JobsDir)) {
    New-Item -ItemType Directory -Path $JobsDir | Out-Null
  }
}

function Read-JsonFile([string]$Path) {
  if (-not (Test-Path -LiteralPath $Path)) { return @() }
  $raw = Get-Content -Raw -Encoding UTF8 -LiteralPath $Path
  if (-not $raw.Trim()) { return @() }
  # 经 ForEach-Object 展开，Windows PowerShell 5.1 下 ConvertFrom-Json 返回的数组也按元素输出
  return @($raw | ConvertFrom-Json | ForEach-Object { $_ })
}

function Write-JsonFile([string]$Path, $Value) {
  ConvertTo-Json -InputObject @($Value) -Depth 6 | Set-Content -LiteralPath $Path -Encoding UTF8
}

function Get-Users { return Read-JsonFile $UsersFile }

function Get-CredentialPath([string]$Name) { return (Join-Path $SecretsDir "$Name.cred.xml") }

function Get-UserCredential([string]$Name) {
  # Export-Clixml 写入的 PSCredential：口令由 DPAPI 按当前用户加密，只能由同一账户在本机解密。
  $path = Get-CredentialPath $Name
  if (-not (Test-Path -LiteralPath $path)) { throw "缺少 $Name 的凭据文件 $path，请先 -Action Create" }
  return (Import-Clixml -LiteralPath $path)
}

function Get-UserRecord([string]$Name) {
  $rec = Get-Users | Where-Object { $_.Name -eq $Name } | Select-Object -First 1
  if (-not $rec) { throw "用户 $Name 不在 $UsersFile 中，请先 -Action Create" }
  return $rec
}

function Get-LocalSid([string]$Name) {
  $acct = New-Object Security.Principal.NTAccount($env:COMPUTERNAME, $Name)
  return $acct.Translate([Security.Principal.SecurityIdentifier]).Value
}

function Test-LocalUserExists([string]$Name) {
  try { [void](Get-LocalSid $Name); return $true } catch { return $false }
}

function Get-GroupMemberSids([string]$GroupSid) {
  # 用 SID 定位组，避免本地化组名。
  $group = Get-CimInstance -ClassName Win32_Group -Filter "LocalAccount=True AND SID='$GroupSid'"
  if (-not $group) { throw "找不到本地组 $GroupSid" }
  $members = Get-CimAssociatedInstance -InputObject $group -ResultClassName Win32_UserAccount -Association Win32_GroupUser
  return @($members | ForEach-Object { $_.SID })
}

function Stop-ProcessTree([int]$ProcessId) {
  if ($ProcessId -le 0) { return }
  $ErrorActionPreference = 'Continue'
  & taskkill.exe /F /T /PID $ProcessId *> $null
}

function Invoke-Native([string]$Exe, [string[]]$NativeArgs, [string]$What) {
  # 局部放宽：Windows PowerShell 5.1 在 EAP=Stop 时会把原生命令的 stderr 当作终止错误
  $ErrorActionPreference = 'Continue'
  $out = & $Exe @NativeArgs 2>&1
  if ($LASTEXITCODE -ne 0) {
    throw "$What 失败（退出码 $LASTEXITCODE）：$($out -join ' ')"
  }
}

$LsaSource = @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;

public static class CaSpikeLsa {
    [StructLayout(LayoutKind.Sequential)]
    private struct LSA_UNICODE_STRING { public ushort Length; public ushort MaximumLength; public IntPtr Buffer; }

    [StructLayout(LayoutKind.Sequential)]
    private struct LSA_OBJECT_ATTRIBUTES {
        public int Length; public IntPtr RootDirectory; public IntPtr ObjectName;
        public uint Attributes; public IntPtr SecurityDescriptor; public IntPtr SecurityQualityOfService;
    }

    [DllImport("advapi32.dll")]
    private static extern uint LsaOpenPolicy(IntPtr systemName, ref LSA_OBJECT_ATTRIBUTES attrs, uint access, out IntPtr handle);
    [DllImport("advapi32.dll")]
    private static extern uint LsaAddAccountRights(IntPtr handle, byte[] sid, LSA_UNICODE_STRING[] rights, uint count);
    [DllImport("advapi32.dll")]
    private static extern uint LsaRemoveAccountRights(IntPtr handle, byte[] sid, [MarshalAs(UnmanagedType.U1)] bool allRights, LSA_UNICODE_STRING[] rights, uint count);
    [DllImport("advapi32.dll")]
    private static extern uint LsaClose(IntPtr handle);
    [DllImport("advapi32.dll")]
    private static extern int LsaNtStatusToWinError(uint status);

    private const uint POLICY_CREATE_ACCOUNT = 0x00000010;
    private const uint POLICY_LOOKUP_NAMES = 0x00000800;
    private const uint STATUS_OBJECT_NAME_NOT_FOUND = 0xC0000034;

    private static IntPtr Open() {
        var attrs = new LSA_OBJECT_ATTRIBUTES();
        attrs.Length = Marshal.SizeOf(typeof(LSA_OBJECT_ATTRIBUTES));
        IntPtr handle;
        uint st = LsaOpenPolicy(IntPtr.Zero, ref attrs, POLICY_CREATE_ACCOUNT | POLICY_LOOKUP_NAMES, out handle);
        if (st != 0) throw new Win32Exception(LsaNtStatusToWinError(st));
        return handle;
    }

    private static byte[] SidBytes(string sid) {
        var s = new SecurityIdentifier(sid);
        var b = new byte[s.BinaryLength];
        s.GetBinaryForm(b, 0);
        return b;
    }

    public static void AddRight(string sid, string right) {
        IntPtr handle = Open();
        IntPtr buf = Marshal.StringToHGlobalUni(right);
        try {
            var u = new LSA_UNICODE_STRING();
            u.Length = (ushort)(right.Length * 2);
            u.MaximumLength = (ushort)((right.Length + 1) * 2);
            u.Buffer = buf;
            uint st = LsaAddAccountRights(handle, SidBytes(sid), new LSA_UNICODE_STRING[] { u }, 1);
            if (st != 0) throw new Win32Exception(LsaNtStatusToWinError(st));
        } finally {
            Marshal.FreeHGlobal(buf);
            LsaClose(handle);
        }
    }

    public static void RemoveAllRights(string sid) {
        IntPtr handle = Open();
        try {
            uint st = LsaRemoveAccountRights(handle, SidBytes(sid), true, null, 0);
            if (st != 0 && st != STATUS_OBJECT_NAME_NOT_FOUND) throw new Win32Exception(LsaNtStatusToWinError(st));
        } finally {
            LsaClose(handle);
        }
    }
}
'@

function Initialize-Lsa {
  if (-not ('CaSpikeLsa' -as [type])) { Add-Type -TypeDefinition $LsaSource -Language CSharp }
}

function Grant-BatchLogon([string]$Sid) {
  $grants = @(Read-JsonFile $GrantsFile)
  if ($grants -contains $Sid) { return }
  Initialize-Lsa
  [CaSpikeLsa]::AddRight($Sid, $BatchRight)
  Write-JsonFile $GrantsFile (@($grants) + $Sid)
  Write-Info "已临时授予 $Sid $BatchRight（计划任务方式需要；Cleanup 时撤销）"
}

# ---------------------------------------------------------------- Create / Show

function Invoke-Create {
  Initialize-StateDir
  $existing = @(Get-Users)
  $records = New-Object System.Collections.Generic.List[object]
  foreach ($r in $existing) { $records.Add($r) }

  foreach ($name in $UserName) {
    if ($existing | Where-Object { $_.Name -eq $name }) {
      Write-Info "$name 已由本脚本创建，跳过"
      continue
    }
    if (Test-LocalUserExists $name) {
      throw "本机已存在用户 $name（不是本脚本创建的），拒绝复用；请先确认后手动删除"
    }
    $password = New-RandomPassword
    # §4.1 第 3 步原文为 `net user <名> <口令> /add`；改用 New-LocalUser，口令不进入进程命令行
    # （PR #46 评审 SR-8）。新用户默认只属于 Users 组。
    New-LocalUser -Name $name -Password $password -AccountNeverExpires -PasswordNeverExpires `
      -UserMayNotChangePassword -Description 'clash-assist CI spike temporary user' | Out-Null
    $sid = Get-LocalSid $name
    # New-LocalUser 在 Windows Server（runner）上不会像 `net user /add` 那样自动加入 Users 组，
    # 这里按 SID 显式加入（避免本地化组名），随后照常断言组成员关系。
    Add-LocalGroupMember -SID $SidUsers -Member $sid
    # 先记录用户（Cleanup 依据 users.json 删除），再写凭据
    $records.Add([pscustomobject]@{
        Name     = $name
        Sid      = $sid
        OwnerKey = Get-OwnerKey $sid
      })
    Write-JsonFile $UsersFile $records.ToArray()
    if (-not (Test-Path -LiteralPath $SecretsDir)) { New-Item -ItemType Directory -Path $SecretsDir | Out-Null }
    $cred = New-Object Management.Automation.PSCredential("$env:COMPUTERNAME\$name", $password)
    $cred | Export-Clixml -LiteralPath (Get-CredentialPath $name)

    $admins = Get-GroupMemberSids $SidAdmins
    $users = Get-GroupMemberSids $SidUsers
    if ($admins -contains $sid) { throw "$name 意外属于 Administrators 组，环境不具代表性" }
    if (-not ($users -contains $sid)) { throw "$name 不在 Users 组" }
    Write-Info "已创建标准用户 $name（$sid）"
  }

  if ($env:GITHUB_OUTPUT) {
    foreach ($r in $records) {
      Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "$($r.Name)_sid=$($r.Sid)" -Encoding UTF8
      Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "$($r.Name)_key=$($r.OwnerKey)" -Encoding UTF8
    }
  }
  Invoke-Show
}

function Invoke-Show {
  Get-Users | Select-Object Name, Sid, OwnerKey | Format-Table -AutoSize | Out-String -Width 400 | Write-Host
}

# ---------------------------------------------------------------- Start / Run / Stop

# 包装脚本由 Windows PowerShell 5.1（powershell.exe）以目标身份执行，内容须保持纯 ASCII。
$WrapperSource = @'
param([Parameter(Mandatory = $true)][string]$JobDir)
$ErrorActionPreference = 'Stop'
$spec = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $JobDir 'spec.json') | ConvertFrom-Json
try {
  & whoami.exe /user /groups /fo list 2>&1 | Set-Content -LiteralPath (Join-Path $JobDir 'identity.txt') -Encoding UTF8
  if ($null -ne $spec.Environment) {
    foreach ($p in $spec.Environment.PSObject.Properties) {
      [Environment]::SetEnvironmentVariable($p.Name, [string]$p.Value, 'Process')
    }
  }
  $startArgs = @{
    FilePath               = $spec.FilePath
    WorkingDirectory       = $JobDir
    RedirectStandardOutput = (Join-Path $JobDir 'stdout.txt')
    RedirectStandardError  = (Join-Path $JobDir 'stderr.txt')
    NoNewWindow            = $true
    PassThru               = $true
  }
  if ($spec.Arguments) { $startArgs.ArgumentList = $spec.Arguments }
  $proc = Start-Process @startArgs
  $null = $proc.Handle   # cache the handle so ExitCode is available under Windows PowerShell 5.1
  Set-Content -LiteralPath (Join-Path $JobDir 'pid.txt') -Value $proc.Id -Encoding ASCII
  $proc.WaitForExit()
  Set-Content -LiteralPath (Join-Path $JobDir 'exitcode.txt') -Value $proc.ExitCode -Encoding ASCII
} catch {
  Set-Content -LiteralPath (Join-Path $JobDir 'wrapper-error.txt') -Value ($_ | Out-String) -Encoding UTF8
  if (-not (Test-Path -LiteralPath (Join-Path $JobDir 'pid.txt'))) {
    Set-Content -LiteralPath (Join-Path $JobDir 'pid.txt') -Value 0 -Encoding ASCII
  }
  Set-Content -LiteralPath (Join-Path $JobDir 'exitcode.txt') -Value 125 -Encoding ASCII
}
'@

function Get-JobDir([string]$Name) { return (Join-Path $JobsDir $Name) }

function Wait-ForFile([string]$Path, [int]$Seconds) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  while ((Get-Date) -lt $deadline) {
    if (Test-Path -LiteralPath $Path) { return $true }
    Start-Sleep -Milliseconds 250
  }
  return $false
}

function Get-WrapperCommandLine([string]$JobDir) {
  $ps = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
  $wrapper = Join-Path $JobDir 'wrapper.ps1'
  return @{
    Exe  = $ps
    Args = (Join-WindowsArguments @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
        '-File', $wrapper, '-JobDir', $JobDir))
  }
}

function Start-ViaCredential($Rec, [string]$JobDir) {
  $svc = Get-Service -Name seclogon -ErrorAction SilentlyContinue
  if ($svc -and $svc.Status -ne 'Running') {
    if ($svc.StartType -eq 'Disabled') { Set-Service -Name seclogon -StartupType Manual }
    Start-Service -Name seclogon
  }
  $cred = Get-UserCredential $Rec.Name
  $cmd = Get-WrapperCommandLine $JobDir
  $p = Start-Process -FilePath $cmd.Exe -ArgumentList $cmd.Args -Credential $cred -LoadUserProfile `
    -WorkingDirectory $JobDir -WindowStyle Hidden -PassThru
  return $p.Id
}

function Start-ViaScheduledTask([string]$Identity, $Rec, [string]$JobDir, [string]$JobLabel) {
  $cmd = Get-WrapperCommandLine $JobDir
  $taskName = "$TaskPrefix$JobLabel"
  $taskAction = New-ScheduledTaskAction -Execute $cmd.Exe -Argument $cmd.Args -WorkingDirectory $JobDir
  # 30 分钟，与 spike job 的 timeout-minutes 对齐（PR #46 评审 SR-11 / SR2-4）
  $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit (New-TimeSpan -Minutes $TaskTimeLimitMin) -MultipleInstances IgnoreNew
  if ($Identity -eq 'SYSTEM') {
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    Register-ScheduledTask -TaskName $taskName -Action $taskAction -Principal $principal -Settings $settings -Force | Out-Null
  } else {
    Grant-BatchLogon $Rec.Sid
    $cred = Get-UserCredential $Rec.Name
    # Register-ScheduledTask 只接受字符串口令：仅在本次调用的内存中解出，经 CIM 传给任务计划程序，
    # 不出现在命令行、日志或磁盘上。
    Register-ScheduledTask -TaskName $taskName -Action $taskAction -Settings $settings -RunLevel Limited `
      -User $cred.UserName -Password $cred.GetNetworkCredential().Password -Force | Out-Null
  }
  Start-ScheduledTask -TaskName $taskName | Out-Null
  return $taskName
}

function Invoke-Start {
  if (-not $As) { throw '-As 必填（用户名或 SYSTEM）' }
  if (-not $FilePath) { throw '-FilePath 必填' }
  Initialize-StateDir
  if (-not $Label) { $script:Label = 'job-' + [Guid]::NewGuid().ToString('N').Substring(0, 8) }
  $jobDir = Get-JobDir $Label
  if (Test-Path -LiteralPath $jobDir) { throw "作业 $Label 已存在：$jobDir" }
  New-Item -ItemType Directory -Path $jobDir | Out-Null

  $rec = $null
  if ($As -eq 'SYSTEM') {
    Set-PrivateAcl -Path $jobDir
  } else {
    $rec = Get-UserRecord $As
    Set-PrivateAcl -Path $jobDir -ModifySids @($rec.Sid)
  }

  foreach ($src in $StagePath) {
    Copy-Item -LiteralPath $src -Destination $jobDir -Recurse -Force
  }
  $resolved = if ([IO.Path]::IsPathRooted($FilePath)) { $FilePath } else { Join-Path $jobDir $FilePath }

  $envMap = [ordered]@{}
  foreach ($k in $Environment.Keys) { $envMap[[string]$k] = [string]$Environment[$k] }
  $spec = [ordered]@{
    FilePath    = $resolved
    Arguments   = (Join-WindowsArguments $ArgumentList)
    Environment = $envMap
  }
  ConvertTo-Json -InputObject $spec -Depth 4 | Set-Content -LiteralPath (Join-Path $jobDir 'spec.json') -Encoding UTF8
  Set-Content -LiteralPath (Join-Path $jobDir 'wrapper.ps1') -Value $WrapperSource -Encoding ASCII

  $pidFile = Join-Path $jobDir 'pid.txt'
  $used = $null
  $taskName = $null
  $wrapperPid = $null

  if ($As -ne 'SYSTEM' -and ($Method -eq 'Auto' -or $Method -eq 'Credential')) {
    try {
      $wrapperPid = Start-ViaCredential $rec $jobDir
      if (Wait-ForFile $pidFile $StartWaitSec) {
        $used = 'Credential'
      } else {
        Write-Info "Credential 方式 $StartWaitSec 秒内未启动目标进程"
        if ($wrapperPid) { Stop-ProcessTree $wrapperPid }
      }
    } catch {
      Write-Info "Credential 方式失败：$($_.Exception.Message)"
    }
    if (-not $used -and $Method -eq 'Credential') { throw "以 $As 身份启动失败（Credential）" }
  }

  if (-not $used) {
    $taskName = Start-ViaScheduledTask $As $rec $jobDir $Label
    if (-not (Wait-ForFile $pidFile $TaskStartWaitSec)) {
      throw "以 $As 身份启动失败（ScheduledTask，$TaskStartWaitSec 秒内无 pid.txt）；任务状态：$((Get-ScheduledTask -TaskName $taskName).State)"
    }
    $used = 'ScheduledTask'
  }

  $targetPid = [int]((Get-Content -LiteralPath $pidFile -Raw).Trim())
  $job = [pscustomobject]@{
    Label      = $Label
    As         = $As
    Method     = $used
    JobDir     = $jobDir
    Pid        = $targetPid
    WrapperPid = $wrapperPid
    TaskName   = $taskName
  }
  Write-JsonFile (Join-Path $jobDir 'job.json') $job
  Write-Info "作业 $Label：以 $As 身份经 $used 启动，PID $targetPid"
  return $job
}

function Show-JobOutput([string]$JobDir) {
  foreach ($f in @('identity.txt', 'stdout.txt', 'stderr.txt', 'wrapper-error.txt')) {
    $p = Join-Path $JobDir $f
    if (Test-Path -LiteralPath $p) {
      Write-Host "----- $f -----"
      Get-Content -LiteralPath $p | Write-Host
    }
  }
}

function Stop-JobProcess($Job) {
  foreach ($procId in @($Job.Pid, $Job.WrapperPid)) {
    if ($procId) { Stop-ProcessTree ([int]$procId) }
  }
  if ($Job.TaskName) {
    Stop-ScheduledTask -TaskName $Job.TaskName -ErrorAction SilentlyContinue
  }
}

function Invoke-Run {
  $job = Invoke-Start
  $exitFile = Join-Path $job.JobDir 'exitcode.txt'
  if (-not (Wait-ForFile $exitFile $TimeoutSec)) {
    Write-Info "作业 $($job.Label) 超过 $TimeoutSec 秒未退出，结束进程"
    Stop-JobProcess $job
    Show-JobOutput $job.JobDir
    exit 124
  }
  Start-Sleep -Milliseconds 300   # 等包装脚本写完输出文件
  Show-JobOutput $job.JobDir
  $code = [int]((Get-Content -LiteralPath $exitFile -Raw).Trim())
  Write-Info "作业 $($job.Label) 退出码 $code"
  exit $code
}

function Get-AllJobs {
  if (-not (Test-Path -LiteralPath $JobsDir)) { return @() }
  return @(Get-ChildItem -LiteralPath $JobsDir -Directory | ForEach-Object {
      $f = Join-Path $_.FullName 'job.json'
      if (Test-Path -LiteralPath $f) { Read-JsonFile $f }
    })
}

function Invoke-Stop {
  if (-not $Label) { throw '-Label 必填' }
  $f = Join-Path (Get-JobDir $Label) 'job.json'
  if (-not (Test-Path -LiteralPath $f)) { throw "找不到作业 $Label" }
  $job = @(Read-JsonFile $f)[0]
  Stop-JobProcess $job
  Write-Info "已结束作业 $Label"
}

# ---------------------------------------------------------------- Cleanup

# 配置文件删不掉时的诊断（只读）：是否仍加载、谁还以该 SID 运行、哪个配置单元文件被占用
function Get-ProfileDiagnostics([string]$Sid) {
  $parts = New-Object System.Collections.Generic.List[string]
  $path = $null
  try {
    $prof = Get-CimInstance -ClassName Win32_UserProfile -Filter "SID='$Sid'"
    if ($prof) {
      $path = $prof.LocalPath
      $ref = $prof.PSObject.Properties['RefCount']
      $parts.Add("Loaded=$($prof.Loaded) RefCount=$(if ($ref) { $ref.Value }) Path=$path")
    } else { $parts.Add('无 Win32_UserProfile 记录') }
  } catch { $parts.Add("Win32_UserProfile：$($_.Exception.Message)") }
  $parts.Add("HKU 已加载=$(Test-Path -LiteralPath "Registry::HKEY_USERS\$Sid")")
  try {
    $procs = @(foreach ($p in Get-CimInstance -ClassName Win32_Process) {
        $o = Invoke-CimMethod -InputObject $p -MethodName GetOwnerSid -ErrorAction SilentlyContinue
        if ($o -and $o.ReturnValue -eq 0 -and $o.Sid -eq $Sid) { "$($p.Name)#$($p.ProcessId)" }
      })
    $parts.Add("该 SID 的进程=$($procs -join ',')")
  } catch { $parts.Add("枚举进程：$($_.Exception.Message)") }
  if ($path -and (Test-Path -LiteralPath $path)) {
    $locked = @(foreach ($f in @(Get-ChildItem -LiteralPath $path -Recurse -Force -File -ErrorAction SilentlyContinue)) {
        try { $fs = [IO.File]::Open($f.FullName, 'Open', 'Read', 'None'); $fs.Dispose() } catch { $f.FullName.Substring($path.Length) }
      })
    $parts.Add("被占用的文件（$($locked.Count)）=$(($locked | Select-Object -First 15) -join ',')")
  }
  return ($parts -join '; ')
}

function Invoke-Cleanup {
  $failures = New-Object System.Collections.Generic.List[string]

  foreach ($job in Get-AllJobs) {
    try { Stop-JobProcess $job } catch { $failures.Add("结束作业 $($job.Label)：$($_.Exception.Message)") }
  }

  # 用户名下的残留进程（例如子进程）
  $users = @(Get-Users)
  $names = @($UserName) + @($users | ForEach-Object { $_.Name }) | Sort-Object -Unique
  try {
    foreach ($p in Get-CimInstance -ClassName Win32_Process) {
      $owner = Invoke-CimMethod -InputObject $p -MethodName GetOwner -ErrorAction SilentlyContinue
      if ($owner -and $owner.ReturnValue -eq 0 -and $names -contains $owner.User) {
        Stop-ProcessTree ([int]$p.ProcessId)
      }
    }
  } catch { $failures.Add("枚举残留进程：$($_.Exception.Message)") }

  foreach ($t in @(Get-ScheduledTask -ErrorAction SilentlyContinue | Where-Object { $_.TaskName -like "$TaskPrefix*" })) {
    try { Unregister-ScheduledTask -TaskName $t.TaskName -Confirm:$false } catch { $failures.Add("删除计划任务 $($t.TaskName)：$($_.Exception.Message)") }
  }

  $grants = @(Read-JsonFile $GrantsFile)
  if ($grants.Count -gt 0) {
    try {
      Initialize-Lsa
      foreach ($sid in $grants) { [CaSpikeLsa]::RemoveAllRights($sid) }
    } catch { $failures.Add("撤销登录权限：$($_.Exception.Message)") }
  }

  foreach ($name in $names) {
    $sid = $null
    if (Test-LocalUserExists $name) {
      $sid = Get-LocalSid $name
      $known = $users | Where-Object { $_.Name -eq $name }
      if (-not $known) {
        Write-Info "跳过 $name：不是本脚本创建的用户（$UsersFile 中无记录）"
        continue
      }
      try { Invoke-Native 'net.exe' @('user', $name, '/delete') "net user $name /delete" } catch { $failures.Add($_.Exception.Message) }
    } else {
      $known = $users | Where-Object { $_.Name -eq $name } | Select-Object -First 1
      if ($known) { $sid = $known.Sid }
    }
    if ($sid) {
      # 进程刚被结束时，用户配置单元（NTUSER.DAT 等）要等用户配置文件服务在登录会话结束后异步卸载，
      # 立即删除可能报“文件正被另一进程使用”（MUST-4 回放 run 41145959660）；重试约 10 秒，仍失败则输出诊断
      $profileErr = $null
      for ($i = 0; $i -lt 5; $i++) {
        try {
          Get-CimInstance -ClassName Win32_UserProfile -Filter "SID='$sid'" | Remove-CimInstance -ErrorAction Stop
          $profileErr = $null
          break
        } catch {
          $profileErr = $_.Exception.Message
          Start-Sleep -Seconds 2
        }
      }
      if ($profileErr) {
        Write-Info "配置文件 $name 诊断：$(Get-ProfileDiagnostics $sid)"
        $failures.Add("删除 $name 的配置文件：$profileErr")
      }
    }
  }

  if (Test-Path -LiteralPath $StateDir) {
    try { Remove-Item -LiteralPath $StateDir -Recurse -Force } catch { $failures.Add("删除状态目录：$($_.Exception.Message)") }
  }

  if ($failures.Count -gt 0) {
    foreach ($f in $failures) { Write-Warning $f }
    exit 1
  }
  Write-Info '清理完成'
  # 显式退出码：否则 $LASTEXITCODE 可能残留上面 taskkill 的非零值，调用方（spike workflow）会误判
  exit 0
}

# ---------------------------------------------------------------- main

Assert-Windows
Assert-Admin
switch ($Action) {
  'Create' { Invoke-Create }
  'Show' { Invoke-Show }
  'Start' { Invoke-Start }
  'Run' { Invoke-Run }
  'Stop' { Invoke-Stop }
  'Cleanup' { Invoke-Cleanup }
}
