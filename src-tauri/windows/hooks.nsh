; Luna's own steps in the NSIS installer (tauri.conf.json → installerHooks).

!macro NSIS_HOOK_PREUNINSTALL
  ; The sessions run in Luna's core, a process started from a copy of the exe
  ; under the data folder. Left running, it would keep its sessions going with
  ; no Luna to reach them, and hold files the uninstaller has to remove. Asked
  ; to stop, it has each session quit first; this waits until it is gone.
  nsExec::Exec '"$INSTDIR\${MAINBINARYNAME}.exe" --stop-core'
  Pop $0
  ; The copies of the exe the core ran from are program, not data: they go
  ; with it. They sit in the data folder, which is the install folder when
  ; that is writable and %LOCALAPPDATA%\luna otherwise.
  RMDir /r "$INSTDIR\core"
  RMDir /r "$LOCALAPPDATA\luna\core"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; "Delete the application data" clears what Tauri keeps under the app's
  ; identifier; the WebView's profile — chats, layout — is in the data folder
  ; instead, so it goes here. Not on an update, which uninstalls first.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    RMDir /r "$INSTDIR\webview"
    RMDir /r "$LOCALAPPDATA\luna\webview"
  ${EndIf}
!macroend
