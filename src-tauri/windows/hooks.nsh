; Clash Assist NSIS installer hooks (windows-plan r4 section 3, S-12).
; Included by the Tauri NSIS template via bundle.windows.nsis.installerHooks.
; Keep this file ASCII-only.
;
; Variables used below are defined by the Tauri template (checked against
; tauri-cli v2.11.4, crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi):
;   $DeleteAppDataCheckboxState - 1 only when the user ticked "delete app data"
;                                 on the uninstall page (always 0 for /S).
;   $UpdateMode                 - 1 when run with /UPDATE (one-click update).
;
; Only NSIS_HOOK_POSTUNINSTALL does anything. It removes our own data dir
; %LOCALAPPDATA%\ClashAssist\ (logs\, updates\, update-pending.json), and never
; during an update. It must not touch Clash Verge's directories and must not
; stop verge-mihomo.exe. RmDir /r does not follow junctions / symlinks out of
; the directory; scripts/test-nsis-hooks.ps1 proves this on the Windows runner.

!macro NSIS_HOOK_PREINSTALL
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    ; Never let an empty $LOCALAPPDATA turn this into "\ClashAssist" at the drive root.
    ${If} $LOCALAPPDATA != ""
      RmDir /r "$LOCALAPPDATA\ClashAssist"
    ${EndIf}
  ${EndIf}
!macroend
