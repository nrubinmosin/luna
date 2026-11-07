; Luna's own steps in the NSIS installer (tauri.conf.json → installerHooks).

; The install folder is also Luna's data folder wherever it is writable: the
; sessions' history (agents.jsonl), the log, settings, attachments, the
; WebView's profile. Left to inherit, it takes the ACL of the folder it is in,
; which on a second drive commonly lets every signed-in user read all of that,
; replace the exe or plant a DLL beside it. So before a file goes in (the
; template runs this hook after SetOutPath, before CheckIfAppIsRunning and the
; first File), and before anything installed runs, the folder gets a DACL of
; its own: the installing user, SYSTEM and Administrators in full, nothing
; inherited, everything in it reset to inherit that. By SID, so no account
; name in any language is involved. Luna checks the same as its core starts
; (paths.rs `protect_data_dir`), for a copy installed before this hook.
!macro NSIS_HOOK_PREINSTALL
  Call LunaClaimInstallDir
!macroend

; Stops the installation with `$R9` said.
!macro LunaRefuse
  MessageBox MB_ICONSTOP|MB_OK "$R9" /SD IDOK
  DetailPrint "$R9"
  SetErrorLevel 2
  Abort "$R9"
!macroend

; icacls with these arguments on the install folder; a failure stops the
; installation, its exit code said.
!macro LunaIcacls ARGS
  nsExec::ExecToLog '"$SYSDIR\icacls.exe" ${ARGS}'
  Pop $R8
  ${If} $R8 != 0
    StrCpy $R9 "Could not make $INSTDIR private to you (icacls exit code $R8). Luna was not installed."
    !insertmacro LunaRefuse
  ${EndIf}
!macroend

Function LunaClaimInstallDir
  ; The template's own steps after this hook keep their registers.
  Push $R0
  Push $R1
  Push $R2
  Push $R3
  Push $R4
  Push $R5
  Push $R6
  Push $R7
  Push $R8
  Push $R9
  ; Not a drive's root, whose ACL is the whole drive's.
  StrLen $R0 "$INSTDIR"
  ${GetParent} "$INSTDIR" $R1
  ${If} $R0 <= 3
  ${OrIf} $R1 == ""
    StrCpy $R9 "$INSTDIR is the root of a drive. Choose a folder of its own for Luna."
    !insertmacro LunaRefuse
  ${EndIf}

  ; No link or junction at the folder or on the way to it: a DACL set there
  ; would land wherever it points.
  StrCpy $R1 "$INSTDIR"
  ${Do}
    StrLen $R0 "$R1"
    ${IfThen} $R0 <= 2 ${|} ${ExitDo} ${|}
    System::Call 'kernel32::GetFileAttributesW(w R1) i .R2'
    ${If} $R2 <> -1
      IntOp $R3 $R2 & 0x400 ; FILE_ATTRIBUTE_REPARSE_POINT
      ${If} $R3 <> 0
        StrCpy $R9 "$R1 is a link or junction. Choose a folder that is not reached through one."
        !insertmacro LunaRefuse
      ${EndIf}
    ${EndIf}
    ${GetParent} "$R1" $R1
  ${LoopUntil} $R1 == ""

  ; Empty, or Luna already: a folder holding anything else is someone's, and
  ; making it private would take it away from them.
  StrCpy $R4 0
  FindFirst $R5 $R6 "$INSTDIR\*"
  ${DoWhile} $R6 != ""
    ${If} $R6 != "."
    ${AndIf} $R6 != ".."
      StrCpy $R4 1
    ${EndIf}
    FindNext $R5 $R6
  ${Loop}
  FindClose $R5
  ${If} $R4 = 1
    ${IfNot} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    ${OrIfNot} ${FileExists} "$INSTDIR\uninstall.exe"
      StrCpy $R9 "$INSTDIR already holds files that are not Luna's. Choose an empty folder for Luna."
      !insertmacro LunaRefuse
    ${EndIf}
  ${EndIf}

  ; The installing user's SID, from `"machine\name","S-1-5-…"`: a name holds
  ; no quote, so the SID is what follows the last `","`.
  nsExec::ExecToStack '"$SYSDIR\whoami.exe" /user /fo csv /nh'
  Pop $R0
  Pop $R1
  StrCpy $R7 ""
  ${If} $R0 == 0
    StrCpy $R2 0
    ${Do}
      StrCpy $R3 $R1 4 $R2
      ${IfThen} $R3 == "" ${|} ${ExitDo} ${|}
      ${If} $R3 == '","S'
        IntOp $R2 $R2 + 3
        ${Do}
          StrCpy $R3 $R1 1 $R2
          ${IfThen} $R3 == '"' ${|} ${ExitDo} ${|}
          ${IfThen} $R3 == "" ${|} ${ExitDo} ${|}
          StrCpy $R7 "$R7$R3"
          IntOp $R2 $R2 + 1
        ${Loop}
        ${ExitDo}
      ${EndIf}
      IntOp $R2 $R2 + 1
    ${Loop}
  ${EndIf}
  StrCpy $R3 $R7 4
  ${If} $R3 != "S-1-"
    StrCpy $R9 "Could not tell which user is installing (whoami exit code $R0). Luna was not installed."
    !insertmacro LunaRefuse
  ${EndIf}

  ; Its own explicit entries go, then what it inherits, and these three are
  ; all it has; the folder is the installing user's too, so no one else keeps
  ; an owner's say over it. /L: a link inside is changed itself, never what it
  ; points at.
  !insertmacro LunaIcacls '"$INSTDIR" /reset /L /Q'
  !insertmacro LunaIcacls '"$INSTDIR" /inheritance:r /grant:r "*$R7:(OI)(CI)F" "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" /L /Q'
  !insertmacro LunaIcacls '"$INSTDIR" /setowner "*$R7" /L /Q'
  ${If} $R4 = 1
    !insertmacro LunaIcacls '"$INSTDIR\*" /reset /T /C /L /Q'
  ${EndIf}
  Pop $R9
  Pop $R8
  Pop $R7
  Pop $R6
  Pop $R5
  Pop $R4
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $R0
FunctionEnd

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
