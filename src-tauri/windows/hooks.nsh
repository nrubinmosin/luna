; installer.nsi uses these hooks at the same points as the Tauri installer
; shipped with Luna 0.19.

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
  ; The template includes this file before it defines MAINBINARYNAME, so a
  ; Function here, compiled where it stands, cannot name it; a macro is
  ; expanded where it is inserted, after the defines.
  StrCpy $LunaExeName "${MAINBINARYNAME}.exe"
  StrCpy $LunaUninstKey "${UNINSTKEY}"
  StrCpy $LunaVersion "${VERSION}"
  Call LunaClaimInstallDir
!macroend

Var LunaExeName
Var LunaUninstKey
Var LunaVersion

; A define or variable that does not resolve in Luna's own steps fails the
; build (warning 6000, "unknown variable/constant"), rather than leave the
; text as it is: that once made the existence check below never match.
!pragma warning push
!pragma warning error 6000

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

; What makes $INSTDIR no place for Luna as a path, in $R9 (empty when
; nothing does): a drive's root, whose ACL is the whole drive's, or a link or
; junction at the folder or on the way to it, through which a DACL set there
; would land wherever it points.
Function LunaPathProblem
  Push $R0
  Push $R1
  Push $R2
  Push $R3
  StrCpy $R9 ""
  StrLen $R0 "$INSTDIR"
  ${GetParent} "$INSTDIR" $R1
  ${If} $R0 <= 3
  ${OrIf} $R1 == ""
    StrCpy $R9 "$INSTDIR is the root of a drive. Choose a folder of its own for Luna."
  ${EndIf}
  StrCpy $R1 "$INSTDIR"
  ${Do}
    ${IfThen} $R9 != "" ${|} ${ExitDo} ${|}
    StrLen $R0 "$R1"
    ${IfThen} $R0 <= 2 ${|} ${ExitDo} ${|}
    System::Call 'kernel32::GetFileAttributesW(w R1) i .R2'
    ${If} $R2 <> -1
      IntOp $R3 $R2 & 0x400 ; FILE_ATTRIBUTE_REPARSE_POINT
      ${If} $R3 <> 0
        StrCpy $R9 "$R1 is a link or junction. Choose a folder that is not reached through one."
      ${EndIf}
    ${EndIf}
    ${GetParent} "$R1" $R1
  ${LoopUntil} $R1 == ""
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $R0
FunctionEnd

; Before the template's SetOutPath makes the folder, which it would do
; through a junction: the directory page keeps Next off for such a path. A
; silent install never asks (seen on NSIS 3.11): there the hook below refuses
; it after SetOutPath, which has made the empty folder by then.
Function .onVerifyInstDir
  Push $R9
  Call LunaPathProblem
  ${If} $R9 != ""
    Pop $R9
    Abort
  ${EndIf}
  Pop $R9
FunctionEnd

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
  Call LunaPathProblem
  ${If} $R9 != ""
    !insertmacro LunaRefuse
  ${EndIf}

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
    ${IfNot} ${FileExists} "$INSTDIR\$LunaExeName"
    ${OrIfNot} ${FileExists} "$INSTDIR\uninstall.exe"
      ; Unless it is a repair: the registry's record of the install says it
      ; is in this very folder, of this setup's own version.
      ReadRegStr $R0 SHCTX "$LunaUninstKey" "InstallLocation"
      ReadRegStr $R1 SHCTX "$LunaUninstKey" "DisplayVersion"
      ${If} $R0 != "$\"$INSTDIR$\""
      ${OrIf} $R1 != $LunaVersion
        StrCpy $R9 "$INSTDIR already holds files that are not Luna's. Choose an empty folder for Luna."
        !insertmacro LunaRefuse
      ${EndIf}
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

  ; Luna's note that a pass made its folder private (paths.rs
  ; `verified_marker`) is never taken from a folder that was open: the next
  ; start makes and verifies its own.
  Delete "$INSTDIR\core\private-verified"

  ; Its own explicit entries go, then what it inherits, and these three are
  ; all it has; the folder is the installing user's too, so no one else keeps
  ; an owner's say over it. /L: a link inside is changed itself, never what it
  ; points at.
  !insertmacro LunaIcacls '"$INSTDIR" /reset /L /Q'
  !insertmacro LunaIcacls '"$INSTDIR" /inheritance:r /grant:r "*$R7:(OI)(CI)F" "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" /L /Q'
  !insertmacro LunaIcacls '"$INSTDIR" /setowner "*$R7" /L /Q'
  ${If} $R4 = 1
    !insertmacro LunaIcacls '"$INSTDIR\*" /reset /T /C /L /Q'
    ; And everything in it this user's too: an owner may rewrite the DACL of
    ; what is theirs, whatever the folder says.
    !insertmacro LunaIcacls '"$INSTDIR\*" /setowner "*$R7" /T /C /L /Q'
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
!pragma warning pop

; Whether the uninstaller knows Luna's window closed (LunaCheckIfRunning, in
; installer.nsi) and its core stopped (NSIS_HOOK_PREUNINSTALL): 1 only then.
; The NVIDIA undo waits for both.
Var LunaAppClosed
Var LunaCoreStopped

!macro NSIS_HOOK_PREUNINSTALL
  ; The sessions run in Luna's core, a process started from a copy of the exe
  ; under the data folder. Left running, it would keep its sessions going with
  ; no Luna to reach them, and hold files the uninstaller has to remove. Asked
  ; to stop, it has each session quit first; this waits until it is gone.
  StrCpy $LunaCoreStopped 0
  nsExec::Exec '"$INSTDIR\${MAINBINARYNAME}.exe" --stop-core'
  Pop $0
  ; The copies of the exe the core ran from are program, not data: they go
  ; with it. They sit in the data folder, which is the install folder when
  ; that is writable and %LOCALAPPDATA%\rm-luna otherwise. Only once no core
  ; runs (exit 0): one still running (held for an update under way, which
  ; stops only for the window that asked for it) keeps its folder, whose
  ; core.json is how a window finds it and whose lock keeps a second core
  ; from starting beside it.
  ${If} $0 == 0
    StrCpy $LunaCoreStopped 1
    RMDir /r "$INSTDIR\core"
    RMDir /r "$LOCALAPPDATA\${LOCALDIR}\core"
  ${Else}
    DetailPrint "A Luna core is still running (luna --stop-core: $0); its folder is left in place."
  ${EndIf}
!macroend

; The profile Luna made in NVIDIA's driver profiles for its exe, G-SYNC off
; (src/desktop/gsync.rs), removed, or only Luna's entries in it where someone
; added their own. Run only once the window is known closed and the core
; stopped (un.LunaCheckIfRunning, NSIS_HOOK_PREUNINSTALL), so that neither
; writes the record or the profile meanwhile (the exe's lock keeps a window
; started since from it); else skipped, and said. Not on an update, whose
; Luna keeps it. Where it is not done (the driver asks for administrator
; rights, exit 2; the record is refused or another Luna holds it, 3; anything
; else), the exe's one line says why. What is skipped or not done is said in
; the log and, unless silent or passive, in a message.
Function un.LunaNvidiaUndo
  ${If} $UpdateMode = 1
    Return
  ${EndIf}
  Push $0
  Push $1
  StrCpy $0 0
  ${If} $LunaCoreStopped <> 1
    StrCpy $0 "skipped"
    StrCpy $1 "Luna's G-SYNC profile in NVIDIA's settings is left in place: a Luna core is still running."
  ${ElseIf} $LunaAppClosed <> 1
    StrCpy $0 "skipped"
    StrCpy $1 "Luna's G-SYNC profile in NVIDIA's settings is left in place: whether Luna's window is closed could not be told."
  ${Else}
    nsExec::ExecToStack '"$INSTDIR\${MAINBINARYNAME}.exe" --nvidia-undo'
    Pop $0
    Pop $1
    ${If} $1 == ""
      StrCpy $1 "Luna's G-SYNC profile in NVIDIA's settings may be left in place (luna --nvidia-undo: $0)."
    ${EndIf}
  ${EndIf}
  ${If} $0 != 0
    DetailPrint "$1"
    ${IfNot} ${Silent}
    ${AndIf} $PassiveMode <> 1
      MessageBox MB_ICONINFORMATION|MB_OK "$1"
    ${EndIf}
  ${EndIf}
  Pop $1
  Pop $0
FunctionEnd

!macro NSIS_HOOK_POSTUNINSTALL
  ; "Delete the application data" clears what Tauri keeps under the app's
  ; identifier; the WebView's profile — chats, layout — is in the data folder
  ; instead, so it goes here. Not on an update, which uninstalls first.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    RMDir /r "$INSTDIR\webview"
    RMDir /r "$LOCALAPPDATA\${LOCALDIR}\webview"
  ${EndIf}
!macroend
