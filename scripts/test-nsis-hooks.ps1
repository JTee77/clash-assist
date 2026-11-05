#requires -Version 7
<#
.SYNOPSIS
  在 Windows 上实际执行 src-tauri/windows/hooks.nsh 的卸载删除逻辑（S-12；Ellie PR55；M4b）：
  NSIS_HOOK_POSTUNINSTALL（%LOCALAPPDATA%\ClashAssist），以及自带模板在「删除应用数据」分支里调用的
  CA_DELETE_TAURI_APP_DATA（%APPDATA%\<BUNDLEID>、%LOCALAPPDATA%\<BUNDLEID>）。

.DESCRIPTION
  静默卸载时「删除应用数据」复选框恒为 0，冒烟测试走不到删除分支，所以这里用 makensis
  编一个最小的测试安装器：声明与 Tauri 模板同名的变量 $UpdateMode / $DeleteAppDataCheckboxState，
  安装段只写出卸载器；卸载段（与 Tauri 一样是 un. 上下文）按用例赋值后，照自带模板
  （src-tauri/windows/installer.nsi）的写法在「勾选删除、非 /UPDATE」分支里
  SetShellVarContext current + !insertmacro CA_DELETE_TAURI_APP_DATA，再 !insertmacro NSIS_HOOK_POSTUNINSTALL。
  BUNDLEID 用一次性的测试 id，不碰真实的 com.clashassist.app。三个数据目录同样布置，用例：
    1. 勾选删除、非 /UPDATE：三个目录都被删；目录里指向外部目录的 junction 只删联接本身，
       不跟随进去——外部目录里的哨兵文件必须还在；
    2. 勾选删除、/UPDATE：目录保留；
    3. 未勾选：目录保留；
    4. 勾选删除、非 /UPDATE，但数据目录本身就是指向外部目录的 junction（Ellie r2）：
       只删联接本身，外部目录及其中的文件原样保留；
    5. 同 4，但数据目录本身是目录 symlink（Ellie r3）。建不了 symlink（无权限）时跳过并说明。
  会读写真实的 %LOCALAPPDATA%\ClashAssist（hooks 里写死的路径），所以该目录已存在时拒绝运行。

  M5：另编一个测试安装器（VERSION 固定为 0.3.1），照 installer.nsi 的 Section EarlyChecks 写法执行
  CA_SILENT_COMPARE_INSTALLED_VERSION 后再做上游的「$R0 = -1 则 Abort」。UNINSTKEY 指向一次性的
  HKCU\Software\clash-assist-hooktest-<随机> 键，用例写入不同的 DisplayVersion：
    6. 未安装 / 0.3.0 / 0.3.1：退出码 0，$R0 分别为 1 / 1 / 0；
    7. 0.3.2 / 0.10.0（按数字比较，不按字符串）：静默降级被拒绝，退出码非 0，后续代码不执行。

  用法：pwsh scripts/test-nsis-hooks.ps1 [-HooksFile src-tauri/windows/hooks.nsh] [-MakeNsis <makensis.exe>]
  makensis 查找顺序：-MakeNsis → %LOCALAPPDATA%\tauri\NSIS\makensis.exe（tauri build 下载的版本）→ PATH
  → %ProgramFiles(x86)%\NSIS\makensis.exe（runner 预装）。
  退出码：0 = 全部通过；1 = 有用例失败；2 = 环境不满足。
#>
param(
  [string] $HooksFile = (Join-Path $PSScriptRoot '..' 'src-tauri' 'windows' 'hooks.nsh'),
  [string] $MakeNsis,
  # 断言 makensis 版本（如 v3.11）；release 构建用它确认测的就是打包用的那一份 NSIS
  [string] $ExpectVersion
)
$ErrorActionPreference = 'Stop'

if (-not $IsWindows) { Write-Host '::error::只能在 Windows 上运行'; exit 2 }
$HooksFile = (Resolve-Path -LiteralPath $HooksFile).Path

if (-not $MakeNsis) {
  $cands = @(
    (Join-Path $env:LOCALAPPDATA 'tauri\NSIS\makensis.exe'),
    (Get-Command makensis.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source),
    (Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe')
  ) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) }
  $MakeNsis = $cands | Select-Object -First 1
}
if (-not $MakeNsis) { Write-Host '::error::找不到 makensis.exe'; exit 2 }
$nsisVersion = (& $MakeNsis /VERSION | Out-String).Trim()
Write-Host "makensis：$MakeNsis（$nsisVersion）"
if ($ExpectVersion -and $nsisVersion -ne $ExpectVersion) {
  Write-Host "::error::makensis 版本应为 $ExpectVersion，实际 $nsisVersion"; exit 2
}

$bundle = 'clash-assist-hooktest-' + [guid]::NewGuid().ToString('N').Substring(0, 12)
$data = Join-Path $env:LOCALAPPDATA 'ClashAssist'
$targets = @($data, (Join-Path $env:APPDATA $bundle), (Join-Path $env:LOCALAPPDATA $bundle))
foreach ($t in $targets) {
  if (Test-Path -LiteralPath $t) { Write-Host "::error::$t 已存在，拒绝运行（不覆盖真实数据）"; exit 2 }
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("nsis-hooks-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
# 每个数据目录各自的 junction 目标（outside-i）与「根就是链接」时的目标（root-target-i）
$outside = @(); $rootTarget = @()
for ($i = 0; $i -lt $targets.Count; $i++) {
  $outside += Join-Path $work "outside-$i"
  $rootTarget += Join-Path $work "root-target-$i"
  New-Item -ItemType Directory -Path $outside[$i] | Out-Null
  New-Item -ItemType Directory -Path (Join-Path $rootTarget[$i] 'sub') | Out-Null
}
$rootFiles = @('keep.txt', 'sub\keep2.txt')

$failures = [Collections.Generic.List[string]]::new()
function Check([bool]$cond, [string]$what) {
  if ($cond) { Write-Host "  ok   $what" } else { Write-Host "  FAIL $what"; $failures.Add($what) }
}

function Build-Harness([int]$del, [int]$upd) {
  $nsi = Join-Path $work "hooks-$del-$upd.nsi"
  $exe = Join-Path $work "hooks-$del-$upd.exe"
  $un = Join-Path $work "hooks-$del-$upd-uninstall.exe"
  @"
Unicode true
!include LogicLib.nsh
Name "clash-assist-hooks-test"
OutFile "$exe"
RequestExecutionLevel user
SilentInstall silent
SilentUnInstall silent
!define BUNDLEID "$bundle"
Var UpdateMode
Var DeleteAppDataCheckboxState
!include "$HooksFile"
Section
  WriteUninstaller "$un"
SectionEnd
Section Uninstall
  StrCpy `$DeleteAppDataCheckboxState $del
  StrCpy `$UpdateMode $upd
  ; 与 src-tauri/windows/installer.nsi 的「删除应用数据」分支相同（scripts/check-nsis-template.py 锁定该分支）
  `${If} `$DeleteAppDataCheckboxState = 1
  `${AndIf} `$UpdateMode <> 1
    SetShellVarContext current
    !insertmacro CA_DELETE_TAURI_APP_DATA
  `${EndIf}
  !insertmacro NSIS_HOOK_POSTUNINSTALL
SectionEnd
"@ | Set-Content -LiteralPath $nsi -Encoding utf8BOM
  $out = & $MakeNsis /V2 /WX $nsi 2>&1
  if ($LASTEXITCODE -ne 0) { $out | ForEach-Object { Write-Host "    $_" }; throw "makensis 编译 $nsi 失败（$LASTEXITCODE）" }
  $p = Start-Process -FilePath $exe -ArgumentList '/S' -Wait -PassThru
  if ($p.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $un)) { throw "测试安装器没有写出卸载器（退出码 $($p.ExitCode)）" }
  $un
}

# M5：静默降级检查（CA_SILENT_COMPARE_INSTALLED_VERSION + 上游 EarlyChecks 的 Abort）
function Build-VersionHarness([string]$regKey, [string]$resultFile) {
  $nsi = Join-Path $work 'version.nsi'
  $exe = Join-Path $work 'version.exe'
  $un = Join-Path $work 'version-uninstall.exe'
  @"
Unicode true
!include LogicLib.nsh
!include WordFunc.nsh
Name "clash-assist-version-test"
OutFile "$exe"
RequestExecutionLevel user
SilentInstall silent
SilentUnInstall silent
!define BUNDLEID "$bundle"
!define VERSION "0.3.1"
!define UNINSTKEY "$regKey"
Var UpdateMode
Var DeleteAppDataCheckboxState
!include "$HooksFile"
Section EarlyChecks
  ; 与 src-tauri/windows/installer.nsi 的 Section EarlyChecks 相同（scripts/check-nsis-template.py 锁定该位置）
  !insertmacro CA_SILENT_COMPARE_INSTALLED_VERSION
  `${If} `$R0 = -1
    Abort
  `${EndIf}
  FileOpen `$0 "$resultFile" w
  FileWrite `$0 `$R0
  FileClose `$0
  WriteUninstaller "$un"
SectionEnd
Section Uninstall
  ; 只为让卸载段与两个变量都被引用（/WX 下未用变量是错误）；本用例不运行卸载器
  StrCpy `$DeleteAppDataCheckboxState 0
  StrCpy `$UpdateMode 0
  !insertmacro NSIS_HOOK_POSTUNINSTALL
SectionEnd
"@ | Set-Content -LiteralPath $nsi -Encoding utf8BOM
  $out = & $MakeNsis /V2 /WX $nsi 2>&1
  if ($LASTEXITCODE -ne 0) { $out | ForEach-Object { Write-Host "    $_" }; throw "makensis 编译 $nsi 失败（$LASTEXITCODE）" }
  $exe
}

function Remove-Link([string]$path) {
  # 只删链接本身（不递归、不跟随）
  if (Test-Path -LiteralPath $path) { [IO.Directory]::Delete($path, $false) }
}

function New-DataDir([string]$dir, [string]$out) {
  New-Item -ItemType Directory -Force -Path (Join-Path $dir 'logs') | Out-Null
  New-Item -ItemType Directory -Force -Path (Join-Path $dir 'updates\nested') | Out-Null
  Set-Content -LiteralPath (Join-Path $dir 'logs\app.log') -Value 'log'
  Set-Content -LiteralPath (Join-Path $dir 'updates\nested\pkg.bin') -Value 'pkg'
  Set-Content -LiteralPath (Join-Path $dir 'update-pending.json') -Value '{}'
  Set-ItemProperty -LiteralPath (Join-Path $dir 'update-pending.json') -Name IsReadOnly -Value $true
  Set-Content -LiteralPath (Join-Path $out 'sentinel.txt') -Value 'must survive'
  $j = Join-Path $dir 'logs\junction-out'
  New-Item -ItemType Junction -Path $j -Target $out | Out-Null
  $item = Get-Item -LiteralPath $j -Force
  if ($item.LinkType -ne 'Junction') { throw "创建 junction 失败：$($item.LinkType)" }
}

# 数据目录本身是链接（Junction / SymbolicLink）；建不了时返回 $false
function New-RootLink([string]$dir, [string]$target, [string]$type) {
  foreach ($f in $rootFiles) { Set-Content -LiteralPath (Join-Path $target $f) -Value 'must survive' }
  try { New-Item -ItemType $type -Path $dir -Target $target -ErrorAction Stop | Out-Null } catch { return $false }
  $item = Get-Item -LiteralPath $dir -Force
  if ($item.LinkType -ne $type) { throw "创建 $type 失败：$($item.LinkType)" }
  $true
}

function Clear-DataDir([string]$dir) {
  $item = Get-Item -LiteralPath $dir -Force -ErrorAction SilentlyContinue
  if (-not $item) { return }
  if ($item.LinkType) { Remove-Link $dir; return }
  Remove-Link (Join-Path $dir 'logs\junction-out')
  Get-ChildItem -LiteralPath $dir -Recurse -Force -File | ForEach-Object { $_.IsReadOnly = $false }
  Remove-Item -LiteralPath $dir -Recurse -Force
}

try {
  $cases = @(
    @{ Name = '勾选删除、非 /UPDATE：删除数据目录，不跟随 junction'; Del = 1; Upd = 0; Deleted = $true }
    @{ Name = '勾选删除、/UPDATE：保留数据目录'; Del = 1; Upd = 1; Deleted = $false }
    @{ Name = '未勾选：保留数据目录'; Del = 0; Upd = 0; Deleted = $false }
    @{ Name = '数据目录本身是 junction：只删联接，不碰目标目录'; Del = 1; Upd = 0; Deleted = $true; RootLink = 'Junction' }
    @{ Name = '数据目录本身是目录 symlink：只删链接，不碰目标目录'; Del = 1; Upd = 0; Deleted = $true; RootLink = 'SymbolicLink' }
  )
  $harness = @{}
  foreach ($c in $cases) {
    Write-Host "== $($c.Name)"
    $key = "$($c.Del)-$($c.Upd)"
    if (-not $harness.ContainsKey($key)) { $harness[$key] = Build-Harness $c.Del $c.Upd }
    $un = $harness[$key]
    $skip = $false
    for ($i = 0; $i -lt $targets.Count; $i++) {
      if ($c.RootLink) {
        if (-not (New-RootLink $targets[$i] $rootTarget[$i] $c.RootLink)) { $skip = $true; break }
      } else { New-DataDir $targets[$i] $outside[$i] }
    }
    if ($skip) {
      Write-Host "  skip 当前账户建不了 $($c.RootLink)（需要开发者模式或管理员），跳过此用例"
      foreach ($t in $targets) { Clear-DataDir $t }
      continue
    }
    # _?= 让卸载器就地同步运行（不复制到临时目录再异步重启）
    $p = Start-Process -FilePath $un -ArgumentList '/S', "_?=$work" -Wait -PassThru
    Check ($p.ExitCode -eq 0) "测试卸载器退出码 0（实际 $($p.ExitCode)）"
    for ($i = 0; $i -lt $targets.Count; $i++) {
      $t = $targets[$i]
      if ($c.Deleted) {
        Check (-not (Test-Path -LiteralPath $t)) "$t 已删除"
      } else {
        Check (Test-Path -LiteralPath (Join-Path $t 'logs\app.log')) "$t 保留"
      }
      if ($c.RootLink) {
        foreach ($f in $rootFiles) {
          $k = Join-Path $rootTarget[$i] $f
          Check ((Test-Path -LiteralPath $k) -and ((Get-Content -Raw -LiteralPath $k).Trim() -eq 'must survive')) "$(Split-Path -Leaf $t) 的目标目录里的 $f 原样保留"
        }
      } else {
        Check (Test-Path -LiteralPath (Join-Path $outside[$i] 'sentinel.txt')) "$(Split-Path -Leaf $t) 里 junction 目标的哨兵文件仍在"
        Check (Test-Path -LiteralPath $outside[$i]) "$(Split-Path -Leaf $t) 里 junction 的目标目录仍在"
      }
    }
    foreach ($t in $targets) { Clear-DataDir $t }
  }

  Write-Host '== M5：静默安装的降级检查（CA_SILENT_COMPARE_INSTALLED_VERSION，安装器版本 0.3.1）'
  $regRel = "Software\$bundle"
  $regPath = "HKCU:\$regRel"
  $result = Join-Path $work 'version-result.txt'
  $vexe = Build-VersionHarness $regRel $result
  $vcases = @(
    @{ Installed = $null; Exit = 0; R0 = '1' }
    @{ Installed = '0.3.0'; Exit = 0; R0 = '1' }
    @{ Installed = '0.3.1'; Exit = 0; R0 = '0' }
    @{ Installed = '0.3.2'; Exit = 'nonzero' }
    @{ Installed = '0.10.0'; Exit = 'nonzero' }
  )
  foreach ($v in $vcases) {
    if (Test-Path -LiteralPath $regPath) { Remove-Item -LiteralPath $regPath -Recurse -Force }
    if ($v.Installed) {
      New-Item -Path $regPath -Force | Out-Null
      Set-ItemProperty -LiteralPath $regPath -Name DisplayVersion -Value $v.Installed
    }
    if (Test-Path -LiteralPath $result) { Remove-Item -LiteralPath $result -Force }
    $label = if ($v.Installed) { "已装 $($v.Installed)" } else { '未安装' }
    $p = Start-Process -FilePath $vexe -ArgumentList '/S' -Wait -PassThru
    if ($v.Exit -eq 0) {
      Check ($p.ExitCode -eq 0) "$label：静默安装继续（退出码 $($p.ExitCode)）"
      $r0 = if (Test-Path -LiteralPath $result) { (Get-Content -Raw -LiteralPath $result).Trim() } else { '<无>' }
      Check ($r0 -eq $v.R0) "$label：`$R0 = $($v.R0)（实际 $r0）"
    } else {
      Check ($p.ExitCode -ne 0) "$label：静默降级被拒绝，退出码非 0（实际 $($p.ExitCode)）"
      Check (-not (Test-Path -LiteralPath $result)) "$label：Abort 之后的代码没有执行"
    }
  }
} finally {
  if ($regPath -and (Test-Path -LiteralPath $regPath)) { Remove-Item -LiteralPath $regPath -Recurse -Force }
  foreach ($t in $targets) { Clear-DataDir $t }
  if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}

if ($failures.Count) {
  Write-Host "::error::test-nsis-hooks：$($failures.Count) 项失败"
  $failures | ForEach-Object { Write-Host "  - $_" }
  exit 1
}
Write-Host 'test-nsis-hooks：全部通过'
exit 0
