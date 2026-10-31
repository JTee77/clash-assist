; Clash Assist NSIS installer hooks (windows-plan r4 section 3, S-12).
; Included by the Tauri NSIS template via bundle.windows.nsis.installerHooks.
; Keep this file ASCII-only.
;
; Variables used below are defined by the Tauri template (checked against
; tauri-cli v2.11.4, crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi):
;   $DeleteAppDataCheckboxState - 1 only when the user ticked "delete app data"
;                                 on the uninstall page (always 0 for /S).
;   $UpdateMode                 - 1 when run with /UPDATE (one-click update).
;   ${BUNDLEID}                 - the bundle identifier (com.clashassist.app).
;
; M4b: the app-data deletion of the template itself (RmDir /r on
; $APPDATA\${BUNDLEID} and $LOCALAPPDATA\${BUNDLEID}) is replaced by
; CA_DELETE_TAURI_APP_DATA below, via our copy of the template
; (src-tauri/windows/installer.nsi, bundle.windows.nsis.template).
;
; Only NSIS_HOOK_POSTUNINSTALL runs as a hook. It removes our own data dir
; %LOCALAPPDATA%\ClashAssist\ (logs\, updates\, update-pending.json), and never
; during an update. It must not touch Clash Verge's directories and must not
; stop verge-mihomo.exe.
;
; Do NOT use "RmDir /r" here: NSIS 3.10 follows directory junctions and deletes
; the files behind them (caught by scripts/test-nsis-hooks.ps1 on the Windows
; runner). un.CA_RmTreeNoFollow walks the tree itself and removes any reparse
; point (junction / symlink) as a link only, never descending into it.
; scripts/test-nsis-hooks.ps1 runs this hook in a real uninstaller and checks
; that a file behind a junction survives.

;  Input: directory path on the stack. Deletes its contents and the directory.
;  The path itself is checked first (the root may be a link too): unreadable
;  attributes -> leave it alone; reparse point -> RMDir the link only; not a
;  directory -> leave it alone. Entries inside are handled the same way:
;  reparse points (or entries whose attributes cannot be read) are removed with
;  non-recursive RMDir / Delete, which only drop the link itself.
;  Only called with fixed paths: $LOCALAPPDATA\ClashAssist, $APPDATA\${BUNDLEID}
;  and $LOCALAPPDATA\${BUNDLEID} (and, recursively, plain sub-directories of
;  them); it does not validate the path itself.
Function un.CA_RmTreeNoFollow
  Exch $R0
  Push $R1
  Push $R2
  Push $R3
  System::Call 'kernel32::GetFileAttributesW(w "$R0") i .R3'
  ${If} $R3 = -1
    Goto ca_rmtree_done
  ${EndIf}
  IntOp $R3 $R3 & 0x410 ; REPARSE_POINT | DIRECTORY
  ${If} $R3 = 0x410
    RMDir "$R0" ; link only
    Goto ca_rmtree_done
  ${ElseIf} $R3 <> 0x10
    Goto ca_rmtree_done ; a file or a file symlink: not ours to delete
  ${EndIf}
  FindFirst $R1 $R2 "$R0\*.*"
  ${DoWhile} $R2 != ""
    ${If} $R2 != "."
    ${AndIf} $R2 != ".."
      System::Call 'kernel32::GetFileAttributesW(w "$R0\$R2") i .R3'
      ${If} $R3 = -1
        ; Unknown: treat as a link, never descend.
        RMDir "$R0\$R2"
        Delete "$R0\$R2"
      ${Else}
        IntOp $R3 $R3 & 0x410 ; REPARSE_POINT | DIRECTORY
        ${If} $R3 = 0x10
          Push "$R0\$R2"
          Call un.CA_RmTreeNoFollow
        ${ElseIf} $R3 = 0x410
          RMDir "$R0\$R2"
        ${Else}
          Delete "$R0\$R2"
        ${EndIf}
      ${EndIf}
    ${EndIf}
    FindNext $R1 $R2
  ${Loop}
  FindClose $R1
  RMDir "$R0"
ca_rmtree_done:
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $R0
FunctionEnd

; Used by src-tauri/windows/installer.nsi in place of the template's
;   RmDir /r "$APPDATA\${BUNDLEID}" / RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
; inside its "delete app data ticked and not /UPDATE" branch, right after
; SetShellVarContext current.
!macro CA_DELETE_TAURI_APP_DATA
  ; Never let an empty variable turn these into "\${BUNDLEID}" at the drive root.
  ${If} $APPDATA != ""
    Push "$APPDATA\${BUNDLEID}"
    Call un.CA_RmTreeNoFollow
  ${EndIf}
  ${If} $LOCALAPPDATA != ""
    Push "$LOCALAPPDATA\${BUNDLEID}"
    Call un.CA_RmTreeNoFollow
  ${EndIf}
!macroend

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
      Push "$LOCALAPPDATA\ClashAssist"
      Call un.CA_RmTreeNoFollow
    ${EndIf}
  ${EndIf}
!macroend
