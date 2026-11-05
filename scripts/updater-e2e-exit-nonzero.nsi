; 仅供 updater-e2e 的 installer-exit-nonzero 用例使用的**测试安装程序**（docs/windows-updater.md「测试安装程序」）。
; 不是 Clash Assist 安装包：.onInit 第一条指令就设置非零退出码并退出，
; 不结束任何进程，不写注册表，不创建、修改或删除任何文件或目录（没有任何 Section 指令会执行）。
; 它在 updater-e2e.yml 的 e2e job 中用 runner 的 makensis 现场编译，只放进本地回放服务器的根目录，
; 不进入任何上传的 artifact，也不被 release workflow 引用（selftest job 的隔离检查会验证这两点）。
;
; 用法：makensis /WX /DVERSION=0.3.1 /DOUTFILE=<路径> scripts/updater-e2e-exit-nonzero.nsi
; VERSION 必须是三段版本号，写入 ProductVersion（核心 S-6 逐字比较）。
Unicode true
!ifndef VERSION
  !error "需要 /DVERSION=<x.y.z>"
!endif
!ifndef OUTFILE
  !error "需要 /DOUTFILE=<路径>"
!endif
!define EXIT_CODE 42

Name "CLASH_ASSIST_E2E_TEST_INSTALLER_EXIT_NONZERO"
OutFile "${OUTFILE}"
RequestExecutionLevel user
SilentInstall silent
InstallDir "$TEMP\clash-assist-e2e-test-installer-unused"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "CLASH_ASSIST_E2E_TEST_INSTALLER_EXIT_NONZERO"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "CLASH_ASSIST_E2E_TEST_INSTALLER_EXIT_NONZERO (updater-e2e test only, not a release)"
VIAddVersionKey "Comments" "CLASH_ASSIST_E2E_TEST_INSTALLER_EXIT_NONZERO"
VIAddVersionKey "LegalCopyright" "test only"

Function .onInit
  SetErrorLevel ${EXIT_CODE}
  Quit
FunctionEnd

Section "-unreachable"
SectionEnd
