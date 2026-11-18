; Clipdump hooks for the Tauri NSIS installer (bundle.windows.nsis.installerHooks).
;
; The generated uninstaller shows a checkbox on its confirm page. Its label comes
; from installer/English.nsh ("Also delete clipboard history") and it is unchecked
; by default. Tauri keeps the answer in $DeleteAppDataCheckboxState (1 = checked).
; A silent (/S) or passive (/P) uninstall never shows that page, so the state stays
; 0 and nothing of the user's data is removed. An update (/UPDATE, the uninstall
; step of an upgrade) is excluded as well.
;
; Where Clipdump keeps its files (see src-tauri/src/lib.rs and settings/file.rs):
;   %LOCALAPPDATA%\clipdump   history.db (+ -wal, -shm), blobs\, thumbs\, key.bin,
;                             perf.log, history.corrupt-*.db
;   %APPDATA%\clipdump        settings.json
; Tauri's own checkbox code removes %APPDATA%\<identifier> and
; %LOCALAPPDATA%\<identifier> (com.clipdump.app: the WebView2 profile). That does
; not cover the two folders above, so this hook removes them.
;
; Note: %LOCALAPPDATA%\clipdump is also the default install folder of a per-user
; Tauri install, so the executable and the data share a folder. The uninstaller has
; already deleted the executable and uninstall.exe when this hook runs, and the
; plain RMDIR of the install folder in the template leaves the data alone, which
; is what makes "keep my history" work.

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    ; The login item written by the autostart plugin: the Run value is named after
    ; the product, and the Task Manager "Startup apps" switch lives next to it.
    ; The template removes the Run value too; both deletes are idempotent.
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "${PRODUCTNAME}"

    ${If} $DeleteAppDataCheckboxState = 1
      SetShellVarContext current
      ${If} $LOCALAPPDATA != ""
        RmDir /r "$LOCALAPPDATA\clipdump"
      ${EndIf}
      ${If} $APPDATA != ""
        RmDir /r "$APPDATA\clipdump"
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend
