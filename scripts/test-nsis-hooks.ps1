#requires -Version 7
<#
.SYNOPSIS
  在 Windows 上实际执行 src-tauri/windows/hooks.nsh 的 NSIS_HOOK_POSTUNINSTALL（S-12；Ellie PR55）。

.DESCRIPTION
  静默卸载时「删除应用数据」复选框恒为 0，冒烟测试走不到 RmDir 分支，所以这里用 makensis
  编一个最小的测试安装器：声明与 Tauri 模板同名的变量 $UpdateMode / $DeleteAppDataCheckboxState，
  按用例赋值后 !insertmacro NSIS_HOOK_POSTUNINSTALL，再静默运行。用例：
    1. 勾选删除、非 /UPDATE：%LOCALAPPDATA%\ClashAssist 被删；目录里指向外部目录的 junction
       只删联接本身，不跟随进去——外部目录里的哨兵文件必须还在；
    2. 勾选删除、/UPDATE：目录保留；
    3. 未勾选：目录保留。
  会读写真实的 %LOCALAPPDATA%\ClashAssist（hooks 里写死的路径），所以该目录已存在时拒绝运行。

  用法：pwsh scripts/test-nsis-hooks.ps1 [-HooksFile src-tauri/windows/hooks.nsh] [-MakeNsis <makensis.exe>]
  makensis 查找顺序：-MakeNsis → %LOCALAPPDATA%\tauri\NSIS\makensis.exe（tauri build 下载的版本）→ PATH
  → %ProgramFiles(x86)%\NSIS\makensis.exe（runner 预装）。
  退出码：0 = 全部通过；1 = 有用例失败；2 = 环境不满足。
#>
param(
  [string] $HooksFile = (Join-Path $PSScriptRoot '..' 'src-tauri' 'windows' 'hooks.nsh'),
  [string] $MakeNsis
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
Write-Host "makensis：$MakeNsis（$(& $MakeNsis /VERSION)）"

$data = Join-Path $env:LOCALAPPDATA 'ClashAssist'
if (Test-Path -LiteralPath $data) { Write-Host "::error::$data 已存在，拒绝运行（不覆盖真实数据）"; exit 2 }

$work = Join-Path ([IO.Path]::GetTempPath()) ("nsis-hooks-" + [guid]::NewGuid().ToString('N'))
$outside = Join-Path $work 'outside'
New-Item -ItemType Directory -Path $outside | Out-Null
$sentinel = Join-Path $outside 'sentinel.txt'

$failures = [Collections.Generic.List[string]]::new()
function Check([bool]$cond, [string]$what) {
  if ($cond) { Write-Host "  ok   $what" } else { Write-Host "  FAIL $what"; $failures.Add($what) }
}

function Build-Harness([int]$del, [int]$upd) {
  $nsi = Join-Path $work "hooks-$del-$upd.nsi"
  $exe = Join-Path $work "hooks-$del-$upd.exe"
  @"
Unicode true
!include LogicLib.nsh
Name "clash-assist-hooks-test"
OutFile "$exe"
RequestExecutionLevel user
SilentInstall silent
Var UpdateMode
Var DeleteAppDataCheckboxState
!include "$HooksFile"
Section
  StrCpy `$DeleteAppDataCheckboxState $del
  StrCpy `$UpdateMode $upd
  !insertmacro NSIS_HOOK_POSTUNINSTALL
SectionEnd
"@ | Set-Content -LiteralPath $nsi -Encoding utf8BOM
  $out = & $MakeNsis /V2 /WX $nsi 2>&1
  if ($LASTEXITCODE -ne 0) { $out | ForEach-Object { Write-Host "    $_" }; throw "makensis 编译 $nsi 失败（$LASTEXITCODE）" }
  $exe
}

function Remove-Junction([string]$path) {
  # 只删联接本身（不递归、不跟随）
  if (Test-Path -LiteralPath $path) { [IO.Directory]::Delete($path, $false) }
}

function New-DataDir {
  New-Item -ItemType Directory -Force -Path (Join-Path $data 'logs') | Out-Null
  Set-Content -LiteralPath (Join-Path $data 'logs\app.log') -Value 'log'
  Set-Content -LiteralPath $sentinel -Value 'must survive'
  $j = Join-Path $data 'logs\junction-out'
  New-Item -ItemType Junction -Path $j -Target $outside | Out-Null
  $item = Get-Item -LiteralPath $j -Force
  if ($item.LinkType -ne 'Junction') { throw "创建 junction 失败：$($item.LinkType)" }
  $j
}

function Clear-DataDir {
  if (Test-Path -LiteralPath $data) {
    Remove-Junction (Join-Path $data 'logs\junction-out')
    Remove-Item -LiteralPath $data -Recurse -Force
  }
}

try {
  $cases = @(
    @{ Name = '勾选删除、非 /UPDATE：删除数据目录，不跟随 junction'; Del = 1; Upd = 0; Deleted = $true }
    @{ Name = '勾选删除、/UPDATE：保留数据目录'; Del = 1; Upd = 1; Deleted = $false }
    @{ Name = '未勾选：保留数据目录'; Del = 0; Upd = 0; Deleted = $false }
  )
  foreach ($c in $cases) {
    Write-Host "== $($c.Name)"
    $exe = Build-Harness $c.Del $c.Upd
    $null = New-DataDir
    $p = Start-Process -FilePath $exe -ArgumentList '/S' -Wait -PassThru
    Check ($p.ExitCode -eq 0) "测试安装器退出码 0（实际 $($p.ExitCode)）"
    if ($c.Deleted) {
      Check (-not (Test-Path -LiteralPath $data)) "$data 已删除"
    } else {
      Check (Test-Path -LiteralPath (Join-Path $data 'logs\app.log')) "$data 保留"
    }
    Check (Test-Path -LiteralPath $sentinel) "junction 目标里的哨兵文件仍在（$sentinel）"
    Check (Test-Path -LiteralPath $outside) "junction 目标目录仍在"
    Clear-DataDir
  }
} finally {
  Clear-DataDir
  if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}

if ($failures.Count) {
  Write-Host "::error::test-nsis-hooks：$($failures.Count) 项失败"
  $failures | ForEach-Object { Write-Host "  - $_" }
  exit 1
}
Write-Host 'test-nsis-hooks：全部通过'
exit 0
