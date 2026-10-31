#Requires -Version 7
<#
.SYNOPSIS
  Windows 静默安装 / 启动 / 卸载冒烟测试（windows-plan r4 §6 M4 ①–⑤、⑩；CI 与真机共用）。

.DESCRIPTION
  ① setup.exe /S 退出码为 0；
  ② 从 HKCU Uninstall 键读出 InstallLocation（必须带引号）、MainBinaryName、DisplayVersion，
     断言安装目录为 %LOCALAPPDATA%\Clash Assist、主程序与 uninstall.exe 存在、版本一致；
  ③ 启动主程序，等待 StartupSeconds 秒后进程仍存活，且有 msedgewebview2.exe 子进程；
  ⑩ S-12：在 %LOCALAPPDATA%\ClashAssist 放一个标记文件，以 /S /UPDATE 重装后标记仍在；
     同时静态检查 hooks.nsh 的删除逻辑带 $UpdateMode 与 $LOCALAPPDATA 非空守卫、且不用 RmDir /r（-HooksFile）；
  ④ uninstall.exe /S 后安装目录与 Uninstall 键都已清理；静默卸载不勾选“删除应用数据”，
     标记文件应保留；
  ⑤ 便携 zip 解压到干净目录后启动，同 ③ 的判据（V14：验证是否需要随包提供 WebView2Loader.dll）。

  ③ / ⑤ 的存活判据（M4c）：每 0.5 秒检查一次，StartupSeconds 秒内进程一直存活才算通过；
  中途退出时记录退出时刻与退出码（0xC0000409 等同时以十六进制打印），并（给了 -DiagDir 时）收集诊断。
  中途以退出码 0 正常退出也算失败：GUI 主程序不应自行退出。

  说明：$DeleteAppDataCheckboxState 只能在卸载界面勾选，静默模式下恒为 0，所以“勾选后删除”
  这条路径 CI 无法触发，留给真机清单第 14 项。

  测试前若机器上已装有 Clash Assist，脚本拒绝运行（避免误卸载用户的安装）。

.EXAMPLE
  pwsh -File scripts/smoke-windows.ps1 -Setup dist/Clash.Assist_0.3.0_x64-setup.exe `
       -Portable dist/Clash.Assist_0.3.0_x64-portable.zip -Version 0.3.0 -HooksFile src-tauri/windows/hooks.nsh
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory)] [string] $Setup,
  [Parameter(Mandatory)] [string] $Portable,
  [Parameter(Mandatory)] [string] $Version,
  [string] $HooksFile,
  [int] $StartupSeconds = 15,
  # 诊断输出目录（M4c）：每次启动的 stdout/stderr、进程退出后的 Application 事件日志、应用自己的日志。
  # 设置后同时置 RUST_BACKTRACE=1（子进程继承）。WER LocalDumps 由调用方（CI）配置，转储也应写到这里。
  [string] $DiagDir,
  [int] $UninstallTimeoutSeconds = 120
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ProductName = 'Clash Assist'
$UninstKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$ProductName"
$ExpectedInstallDir = Join-Path $env:LOCALAPPDATA $ProductName
$AppDataDir = Join-Path $env:LOCALAPPDATA 'ClashAssist'
$MarkerPath = Join-Path $AppDataDir 'logs\smoke-windows-marker.txt'

$script:failures = [System.Collections.Generic.List[string]]::new()
function Step([string] $msg) { Write-Host "`n=== $msg" }
function Pass([string] $msg) { Write-Host "  ok   $msg" }
function Fail([string] $msg) { Write-Host "  FAIL $msg"; $script:failures.Add($msg) }
function Assert([bool] $cond, [string] $msg) { if ($cond) { Pass $msg } else { Fail $msg } }

function Get-Descendants([int] $RootPid) {
  $all = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name)
  $out = [System.Collections.Generic.List[object]]::new()
  $queue = [System.Collections.Generic.Queue[int]]::new()
  $queue.Enqueue($RootPid)
  while ($queue.Count -gt 0) {
    $p = $queue.Dequeue()
    foreach ($c in $all | Where-Object { $_.ParentProcessId -eq $p -and $_.ProcessId -ne $p }) {
      $out.Add($c); $queue.Enqueue([int]$c.ProcessId)
    }
  }
  return $out
}

function Stop-Tree([int] $RootPid) {
  $kids = @(Get-Descendants $RootPid)
  Stop-Process -Id $RootPid -Force -ErrorAction SilentlyContinue
  foreach ($k in $kids) { Stop-Process -Id $k.ProcessId -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Seconds 2
}

$AppLogDir = Join-Path $env:APPDATA 'logs\ClashAssist'   # platform::app_log_dir()（dirs::data_dir() = Roaming）
if ($DiagDir) {
  New-Item -ItemType Directory -Force -Path $DiagDir | Out-Null
  $DiagDir = (Resolve-Path -LiteralPath $DiagDir).Path
  $env:RUST_BACKTRACE = '1'
}

# 进程退出后：打印并保存这段时间内与 exe 相关的 Application Error / WER 事件
function Save-CrashEvents([datetime] $Since, [string] $ExeName, [string] $Tag) {
  if (-not $DiagDir) { return }
  Start-Sleep -Seconds 3   # WER 写事件有延迟
  $ev = @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = $Since } -ErrorAction SilentlyContinue |
    Where-Object { $_.ProviderName -in @('Application Error', 'Windows Error Reporting', 'Application Hang', '.NET Runtime', 'SideBySide') -and $_.Message -match [regex]::Escape($ExeName) })
  $text = ($ev | ForEach-Object { "[$($_.TimeCreated.ToString('o'))] $($_.ProviderName) id=$($_.Id)`n$($_.Message)`n" }) -join "`n"
  Set-Content -LiteralPath (Join-Path $DiagDir "$Tag-events.txt") -Value $text -Encoding utf8
  Write-Host "  ---- Application 事件（$($ev.Count) 条，$Tag）----"
  if ($text) { $text -split "`n" | Select-Object -First 80 | ForEach-Object { Write-Host "  | $_" } }
}

function Show-DiagFile([string] $Path, [string] $Title, [int] $Max = 80) {
  if ((Test-Path -LiteralPath $Path) -and (Get-Item -LiteralPath $Path).Length -gt 0) {
    Write-Host "  ---- $Title（$Path）----"
    Get-Content -LiteralPath $Path -Tail $Max | ForEach-Object { Write-Host "  | $_" }
  }
}

# 启动 exe，StartupSeconds 秒内每 0.5 秒检查一次，要求一直存活且有 msedgewebview2.exe 子孙进程；最后结束整棵进程树
function Test-Launch([string] $Exe, [string] $Label, [string] $Tag) {
  $since = Get-Date
  $sp = @{ FilePath = $Exe; WorkingDirectory = (Split-Path $Exe); PassThru = $true }
  if ($DiagDir) {
    $sp.RedirectStandardOutput = Join-Path $DiagDir "$Tag-stdout.txt"
    $sp.RedirectStandardError = Join-Path $DiagDir "$Tag-stderr.txt"
  }
  $p = Start-Process @sp
  Write-Host "  启动 $Label（PID $($p.Id)），观察 $StartupSeconds 秒"
  $sw = [Diagnostics.Stopwatch]::StartNew()
  while ($sw.Elapsed.TotalSeconds -lt $StartupSeconds -and -not $p.HasExited) { Start-Sleep -Milliseconds 500; $p.Refresh() }
  $p.Refresh()
  if ($p.HasExited) {
    $code = $p.ExitCode
    Fail ("$Label 在 {0:N1} 秒时退出（要求 $StartupSeconds 秒内一直运行），退出码 $code（0x{1:X8}）" -f $sw.Elapsed.TotalSeconds, $code)
    Save-CrashEvents $since (Split-Path -Leaf $Exe) $Tag
    if ($DiagDir) {
      Show-DiagFile (Join-Path $DiagDir "$Tag-stderr.txt") "$Tag stderr"
      Show-DiagFile (Join-Path $DiagDir "$Tag-stdout.txt") "$Tag stdout"
    }
    Show-DiagFile (Join-Path $AppLogDir 'app.log') 'app.log（末尾）' 40
    return
  }
  Pass "$Label 启动 $StartupSeconds 秒内一直在运行"
  $wv = @(Get-Descendants $p.Id | Where-Object { $_.Name -ieq 'msedgewebview2.exe' })
  Assert ($wv.Count -gt 0) "$Label 有 msedgewebview2.exe 子进程（$($wv.Count) 个）"
  Stop-Tree $p.Id
}

function Read-UninstallKey {
  if (-not (Test-Path -LiteralPath $UninstKey)) { return $null }
  return Get-ItemProperty -LiteralPath $UninstKey
}

function Wait-Uninstalled([string] $InstallDir) {
  # NSIS 卸载程序会把自己复制到 %TEMP% 再运行，原进程立即返回，所以按结果轮询
  $deadline = (Get-Date).AddSeconds($UninstallTimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    if (-not (Test-Path -LiteralPath $UninstKey) -and -not (Test-Path -LiteralPath (Join-Path $InstallDir 'uninstall.exe'))) { return $true }
    Start-Sleep -Seconds 2
  }
  return $false
}

foreach ($f in @($Setup, $Portable)) {
  if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { throw "找不到文件：$f" }
}
$Setup = (Resolve-Path -LiteralPath $Setup).Path
$Portable = (Resolve-Path -LiteralPath $Portable).Path
if (Read-UninstallKey) { throw "本机已安装 $ProductName（$UninstKey 存在），拒绝运行冒烟测试" }
if (Test-Path -LiteralPath $AppDataDir) { throw "$AppDataDir 已存在，拒绝运行冒烟测试（避免改动已有的应用数据）" }

$installDir = $ExpectedInstallDir
$portableDir = $null
try {
  Step '① 静默安装（/S）'
  $p = Start-Process -FilePath $Setup -ArgumentList '/S' -Wait -PassThru
  Assert ($p.ExitCode -eq 0) "setup.exe /S 退出码为 0（实际 $($p.ExitCode)）"

  Step '② Uninstall 键与安装目录'
  $key = Read-UninstallKey
  Assert ($null -ne $key) "存在 $UninstKey"
  $exe = $null
  if ($key) {
    $loc = [string]$key.InstallLocation
    Assert ($loc.Length -ge 2 -and $loc.StartsWith('"') -and $loc.EndsWith('"')) "InstallLocation 带引号（$loc）"
    $installDir = $loc.Trim('"')
    Assert ([string]::Equals($installDir.TrimEnd('\'), $ExpectedInstallDir, [StringComparison]::OrdinalIgnoreCase)) "安装目录为 $ExpectedInstallDir（实际 $installDir）"
    $main = [string]$key.MainBinaryName
    Assert ($main -like '*.exe') "MainBinaryName = $main"
    Assert ([string]$key.DisplayVersion -eq $Version) "DisplayVersion = $Version（实际 $($key.DisplayVersion)）"
    $exe = Join-Path $installDir $main
    Assert (Test-Path -LiteralPath $exe -PathType Leaf) "主程序存在：$exe"
    Assert (Test-Path -LiteralPath (Join-Path $installDir 'uninstall.exe') -PathType Leaf) 'uninstall.exe 存在'
    $lnk = @(
      (Join-Path ([Environment]::GetFolderPath('Programs')) "$ProductName.lnk"),
      (Join-Path ([Environment]::GetFolderPath('Programs')) "$ProductName\$ProductName.lnk")
    ) | Where-Object { Test-Path -LiteralPath $_ }
    Assert (@($lnk).Count -gt 0) "开始菜单快捷方式存在（$(@($lnk) -join ', ')）"
    $ver = (Get-Item -LiteralPath $exe).VersionInfo.ProductVersion
    Write-Host "  主程序 ProductVersion = $ver"
  }

  Step '③ 启动检查（安装版）'
  if ($exe -and (Test-Path -LiteralPath $exe)) { Test-Launch $exe '安装版' 'installed' } else { Fail '无法定位主程序，跳过启动检查' }

  Step '⑩ S-12：/UPDATE 重装不删应用数据'
  if ($HooksFile) {
    $hooks = Get-Content -Raw -LiteralPath $HooksFile
    $guarded = $hooks -match '(?s)!macro\s+NSIS_HOOK_POSTUNINSTALL.*?\$\{If\}\s+\$DeleteAppDataCheckboxState\s*=\s*1\s*\r?\n\s*\$\{AndIf\}\s+\$UpdateMode\s*<>\s*1.*?\$\{If\}\s+\$LOCALAPPDATA\s*!=\s*""\s*\r?\n\s*Push\s+"\$LOCALAPPDATA\\ClashAssist"\s*\r?\n\s*Call\s+un\.CA_RmTreeNoFollow.*?!macroend'
    Assert $guarded 'hooks.nsh：删除只在勾选删除、非 /UPDATE 且 $LOCALAPPDATA 非空时执行'
    # NSIS 3.10 的 RmDir /r 会跟随 junction（test-nsis-hooks.ps1 实测），hooks 里不得再用
    Assert (-not ($hooks -match '(?im)^\s*RmDir\s+/r\b')) 'hooks.nsh：不使用会跟随 junction 的 RmDir /r'
  } else {
    Write-Host '  （未提供 -HooksFile，跳过静态检查）'
  }
  New-Item -ItemType Directory -Force -Path (Split-Path $MarkerPath) | Out-Null
  Set-Content -LiteralPath $MarkerPath -Value 'smoke-windows' -Encoding utf8
  $p = Start-Process -FilePath $Setup -ArgumentList '/S', '/UPDATE' -Wait -PassThru
  Assert ($p.ExitCode -eq 0) "setup.exe /S /UPDATE 退出码为 0（实际 $($p.ExitCode)）"
  Assert (Test-Path -LiteralPath $MarkerPath) "/UPDATE 重装后 $MarkerPath 仍在"
  Assert ($null -ne (Read-UninstallKey)) '/UPDATE 重装后 Uninstall 键仍在'

  Step '④ 静默卸载（uninstall.exe /S）'
  $uninst = Join-Path $installDir 'uninstall.exe'
  if (Test-Path -LiteralPath $uninst) {
    Start-Process -FilePath $uninst -ArgumentList '/S' -Wait | Out-Null
    Assert (Wait-Uninstalled $installDir) "卸载完成：Uninstall 键已删除、uninstall.exe 已删除（≤ $UninstallTimeoutSeconds 秒）"
    $left = @(Get-ChildItem -LiteralPath $installDir -Force -ErrorAction SilentlyContinue)
    Assert ($left.Count -eq 0) "安装目录已清空（剩余：$(($left | ForEach-Object Name) -join ', ')）"
    Assert (Test-Path -LiteralPath $MarkerPath) '静默卸载（未勾选删除应用数据）保留应用数据'
  } else {
    Fail "找不到 $uninst，无法卸载"
  }

  Step '⑤ 便携版（干净目录）'
  $portableDir = Join-Path ([IO.Path]::GetTempPath()) ("ca-portable-" + [guid]::NewGuid().ToString('N'))
  Expand-Archive -LiteralPath $Portable -DestinationPath $portableDir
  $files = @(Get-ChildItem -LiteralPath $portableDir -Recurse -File | ForEach-Object { $_.FullName.Substring($portableDir.Length + 1) })
  Write-Host "  zip 内容：$($files -join ', ')"
  $pexe = @(Get-ChildItem -LiteralPath $portableDir -Filter '*.exe' -File)
  Assert ($pexe.Count -eq 1) "便携 zip 根目录恰有一个 exe（实际 $($pexe.Count) 个）"
  Assert (Test-Path -LiteralPath (Join-Path $portableDir 'portable.txt')) '便携 zip 含 portable.txt'
  Assert ($null -eq (Read-UninstallKey)) '启动便携版前没有安装版（Uninstall 键不存在）'
  if ($pexe.Count -eq 1) { Test-Launch $pexe[0].FullName '便携版' 'portable' }
}
finally {
  Step '清理'
  if ($DiagDir) {
    foreach ($d in @($AppLogDir, (Join-Path $AppDataDir 'logs'))) {
      if (Test-Path -LiteralPath $d) {
        $dest = Join-Path $DiagDir ('applogs-' + ($d -replace '[:\\]+', '_'))
        Copy-Item -LiteralPath $d -Destination $dest -Recurse -Force -ErrorAction SilentlyContinue
        Write-Host "  已复制 $d → $dest"
      }
    }
  }
  Get-Process -ErrorAction SilentlyContinue | Where-Object {
    $_.Path -and ($_.Path.StartsWith($ExpectedInstallDir, [StringComparison]::OrdinalIgnoreCase) -or
      ($portableDir -and $_.Path.StartsWith($portableDir, [StringComparison]::OrdinalIgnoreCase)))
  } | ForEach-Object { Stop-Tree $_.Id }
  if (Read-UninstallKey) {
    $u = Join-Path $installDir 'uninstall.exe'
    if (Test-Path -LiteralPath $u) { Start-Process -FilePath $u -ArgumentList '/S' -Wait | Out-Null; [void](Wait-Uninstalled $installDir) }
  }
  if ($portableDir) { Remove-Item -LiteralPath $portableDir -Recurse -Force -ErrorAction SilentlyContinue }
  Remove-Item -LiteralPath $AppDataDir -Recurse -Force -ErrorAction SilentlyContinue
}

if ($script:failures.Count -gt 0) {
  Write-Host "`nsmoke-windows：失败 $($script:failures.Count) 项："
  $script:failures | ForEach-Object { Write-Host "  - $_" }
  exit 1
}
Write-Host "`nsmoke-windows：全部通过"
exit 0
