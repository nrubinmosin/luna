; Luna's installer, made without Tauri's bundler (scripts/build-windows.ps1
; -Installer). It installs as the installer Tauri's NSIS template
; made for 0.19 does, so that one installs over the other in place either
; way: the same product name, identifier and publisher, per user into
; %LOCALAPPDATA%\Luna (or wherever the last install went), the same
; uninstall key and install-location key in HKCU, the Start-menu shortcut
; with the AUMID toasts go out under, and the same command line, which a
; 0.19 updater passes as `/P /UPDATE /R /ARGS ...`
; (docs/architecture/desktop-services.md, "Installer"). Luna's own steps,
; the folder's DACL and the core's stop, are hooks.nsh's, as in Tauri's.
;
; What the build passes (makensis -D...):
;   VERSION            x.y.z, or x.y.z-pre (semver, as Cargo.toml has it)
;   VIVERSION          x.y.z.0, the numeric version Windows' version
;                      resource takes
;   MAINBINARYSRCPATH  the luna.exe to install
;   NOTICES            THIRD-PARTY-NOTICES.txt, installed beside it
;   INSTALLERICON      icons/icon.ico
;   OUTFILE            the setup to write
;
; Adapted from Tauri's NSIS template: tauri-bundler 2.10.5 (as the Tauri
; CLI 2.12.5 that built 0.19 embeds it),
; crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi of
; https://github.com/tauri-apps/tauri, Copyright (c) 2017 - Present Tauri
; Apps Contributors, MIT OR Apache-2.0 (third-party-licenses/installer/
; tauri-nsis-template, and the installed THIRD-PARTY-NOTICES.txt). Taken
; from it, some of it word for word: the pages, .onInit's choice of the
; folder, the install and uninstall sections, the macros that set a
; shortcut's AUMID, test its target, unpin it and clear the AUMID's lists,
; the Restart Manager check (CheckIfAppIsRunning there), the uninstall
; page's box for the app data and .onInstSuccess. What was changed, as the
; Apache License asks a modified file to say, is listed here.
;
; Left out from Tauri's: the WebView2 bootstrap (no WebView any more), the
; page offering to uninstall a previous version first (an install always
; goes in place, as an update does), WiX migration (Luna never shipped an
; MSI), file associations and deep links (none), and its helper DLL: the
; version compare and string replace it did are plain NSIS here, and its
; de-elevated start (RunAsUser) is `LunaRunAsUser`, through the System
; plug-in. Luna's: the `/ARGS` tail handed on as it stands
; (`SplitCmdLine`), where the template read it with ${GetOptions}, which
; takes the quotes off a tail that opens and ends with a quoted argument.

Unicode true
ManifestDPIAware true
ManifestDPIAwareness PerMonitorV2
SetCompressor /SOLID lzma

!ifndef VERSION | VIVERSION | MAINBINARYSRCPATH | NOTICES | INSTALLERICON | OUTFILE
  !error "pass VERSION, VIVERSION, MAINBINARYSRCPATH, NOTICES, INSTALLERICON and OUTFILE"
!endif
; The setup reads its command line into a string of NSIS_MAX_STRLEN units,
; and Luna's updater sizes the restart's arguments to 1024
; (desktop/updater.rs SETUP_CMDLINE_MAX): a makensis with shorter strings
; would cut them.
!if ${NSIS_MAX_STRLEN} < 1024
  !error "this setup needs a makensis whose NSIS_MAX_STRLEN is 1024 or more"
!endif

!include MUI2.nsh
!include FileFunc.nsh
!include LogicLib.nsh
!include WordFunc.nsh
!include nsDialogs.nsh
!include x64.nsh
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "Win\RestartManager.nsh"

; Match the product name, identifier, executable name and publisher of
; Luna 0.19 so this installer can find an existing installation.
!define PRODUCTNAME "Luna"
!define MANUFACTURER "luna"
!define BUNDLEID "rm.luna"
!define MAINBINARYNAME "luna"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
!define MANUKEY "Software\${MANUFACTURER}"
!define MANUPRODUCTKEY "${MANUKEY}\${PRODUCTNAME}"

Var PassiveMode
Var UpdateMode
Var NoShortcutMode
Var OldMainBinaryName
Var DeleteAppDataCheckbox
Var DeleteAppDataCheckboxState
; $CMDLINE up to the updater's " /ARGS", and what follows it (SplitCmdLine).
Var CmdHead
Var RestartArgs
; 1 when $CMDLINE filled its string, and so may have been cut: the restart
; then gets no arguments rather than some of them.
Var ArgsCut
; What LunaRunAsUser starts Luna with.
Var RunArgs

!include "hooks.nsh"
!include "version.nsh"

Name "${PRODUCTNAME}"
BrandingText " "
OutFile "${OUTFILE}"
; Not the install folder: .onInit tells a /D= from this and picks the folder.
!define PLACEHOLDER_INSTALL_DIR "placeholder\${PRODUCTNAME}"
InstallDir "${PLACEHOLDER_INSTALL_DIR}"
; Keep the per-user installation used by Luna 0.19.
RequestExecutionLevel user

VIProductVersion "${VIVERSION}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
VIAddVersionKey "LegalCopyright" ""
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

!define MUI_ICON "${INSTALLERICON}"
!define MUI_UNICON "${INSTALLERICON}"
; Where the installer's language is kept, as Tauri's keeps it.
!define MUI_LANGDLL_REGISTRY_ROOT "HKCU"
!define MUI_LANGDLL_REGISTRY_KEY "${MANUPRODUCTKEY}"
!define MUI_LANGDLL_REGISTRY_VALUENAME "Installer Language"

; Installer pages. Passive mode (/P, the updater's) shows only the progress.
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
; The details stay up after the install, for whoever has to tell what failed.
!define MUI_FINISHPAGE_NOAUTOCLOSE
; The readme box is the desktop shortcut's.
!define MUI_FINISHPAGE_SHOWREADME
!define MUI_FINISHPAGE_SHOWREADME_TEXT "$(createDesktop)"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION CreateDesktopShortcut
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_FUNCTION RunMainBinary
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_FINISH

; Uninstaller pages: the confirmation, with a box for the app data.
!define MUI_PAGE_CUSTOMFUNCTION_SHOW un.ConfirmShow
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE un.ConfirmLeave
!define MUI_PAGE_CUSTOMFUNCTION_PRE un.SkipIfPassive
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"
LangString createDesktop ${LANG_ENGLISH} "Create desktop shortcut"
LangString deleteAppData ${LANG_ENGLISH} "Delete the application data"
LangString appRunning ${LANG_ENGLISH} "${PRODUCTNAME} is running. Close it first, then try again."
LangString appRunningOkKill ${LANG_ENGLISH} "${PRODUCTNAME} is running.$\nClick OK to close it."
LangString failedToKillApp ${LANG_ENGLISH} "Could not close ${PRODUCTNAME}. Close it yourself, then try again."
LangString notRestarted ${LANG_ENGLISH} "${PRODUCTNAME} was not started again: it could not be started without administrator rights, or whether it would be could not be told. Start it from the Start menu."
LangString argsCut ${LANG_ENGLISH} "${PRODUCTNAME} is started again without the arguments it ran with: they were too long for this setup's command line."
LangString noPermissionToCloseApp ${LANG_ENGLISH} "${PRODUCTNAME} is running as another user or as administrator, and this setup may not close it. Close it yourself, then try again."

; The shell and registry views of a per-user, 64-bit install.
!macro SetContext
  SetShellVarContext current
  ${If} ${RunningX64}
    SetRegView 64
  ${EndIf}
!macroend

; Stops a running Luna before its exe is replaced, through the Restart
; Manager: asked first in the full UI, done at once when silent or passive
; (the updater's Luna has exited by then, or is about to). The core runs
; from a copy under core\, so this never meets it; hooks.nsh stops it on
; uninstall.
!macro LunaCheckIfRunningFunction UN
Function ${UN}LunaCheckIfRunning
  Push $0
  Push $1
  Push $2
  Push $3
  Push $R0
  !insertmacro RestartManager_StartSession $R0
  ${If} $R0 == ""
    Goto done
  ${EndIf}
  !insertmacro RestartManager_RegisterFile $R0 "$INSTDIR\${MAINBINARYNAME}.exe"
  ${If} $0 <> 0
    Goto end_session
  ${EndIf}
  System::Call 'RSTRTMGR::RmGetList(p R0, *i .r1, *i .r2, p 0, *i .r3) i .r0'
  ; 234, ERROR_MORE_DATA: something has the exe open.
  ${If} $0 <> 234
    Goto end_session
  ${EndIf}
  IntOp $0 $3 & ${RmRebootReasonPermissionDenied}
  ${If} $0 <> 0
    StrCpy $1 "$(noPermissionToCloseApp)"
    Goto refuse
  ${EndIf}
  ${IfNot} ${Silent}
  ${AndIf} $PassiveMode <> 1
    MessageBox MB_OKCANCEL "$(appRunningOkKill)" IDOK kill
    StrCpy $1 "$(appRunning)"
    Goto refuse
  ${EndIf}
  kill:
  ; A passive update runs here with no question asked, and RmForceShutdown
  ; ends Luna whatever it holds: the native window writes its chat list, theme
  ; and place before it starts this setup (native/desktop.rs `install_with`),
  ; and nothing here waits for it.
  System::Call 'RSTRTMGR::RmShutdown(p R0, i ${RmForceShutdown}, p 0) i .r0'
  ${If} $0 = 0
    Goto end_session
  ${EndIf}
  StrCpy $1 "$(failedToKillApp)"
  refuse:
  !insertmacro RestartManager_EndSession $R0
  ${If} ${Silent}
    ; A silent run's caller reads the console, if it has one.
    System::Call 'kernel32::AttachConsole(i -1) i .r0'
    ${If} $0 <> 0
      System::Call 'kernel32::GetStdHandle(i -11) i .r0'
      FileWrite $0 "$1$\n"
    ${EndIf}
  ${EndIf}
  SetErrorLevel 2
  Abort "$1"
  end_session:
  !insertmacro RestartManager_EndSession $R0
  done:
  Pop $R0
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd
!macroend
!insertmacro LunaCheckIfRunningFunction ""
!insertmacro LunaCheckIfRunningFunction "un."

; Gives a shortcut the AUMID Windows matches toasts and the taskbar by.
!macro SetLnkAppUserModelId SHORTCUT
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ShellLink} ${IID_IShellLink} r0 ""
  ${If} $0 P<> 0
    ${IUnknown::QueryInterface} $0 '("${IID_IPersistFile}",.r1)'
    ${If} $1 P<> 0
      ${IPersistFile::Load} $1 '("${SHORTCUT}", ${STGM_READWRITE})'
      ${IUnknown::QueryInterface} $0 '("${IID_IPropertyStore}",.r2)'
      ${If} $2 P<> 0
        System::Call 'Oleaut32::SysAllocString(w "${BUNDLEID}") i.r3'
        System::Call '*${SYSSTRUCT_PROPERTYKEY}(${PKEY_AppUserModel_ID})p.r4'
        System::Call '*${SYSSTRUCT_PROPVARIANT}(${VT_BSTR},,&i4 $3)p.r5'
        ${IPropertyStore::SetValue} $2 '($4,$5)'
        System::Call 'Oleaut32::SysFreeString($3)'
        System::Free $4
        System::Free $5
        ${IPropertyStore::Commit} $2 ""
        ${IUnknown::Release} $2 ""
        ${IPersistFile::Save} $1 '("${SHORTCUT}",1)'
      ${EndIf}
      ${IUnknown::Release} $1 ""
    ${EndIf}
    ${IUnknown::Release} $0 ""
  ${EndIf}
!macroend

; Pushes 1 when the shortcut points at TARGET, 0 otherwise; uses $0-$3.
!macro IsShortcutTarget SHORTCUT TARGET
  StrCpy $3 0
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ShellLink} ${IID_IShellLink} r0 ""
  ${If} $0 P<> 0
    ${IUnknown::QueryInterface} $0 '("${IID_IPersistFile}",.r1)'
    ${If} $1 P<> 0
      ${IPersistFile::Load} $1 '("${SHORTCUT}", ${STGM_READ})'
      System::Alloc 520
      Pop $2
      ; 0x4, SLGP_RAWPATH: the path as stored, not resolved.
      ${IShellLink::GetPath} $0 '(.r2, 260, 0, 0x4)'
      ${If} $2 == "${TARGET}"
        StrCpy $3 1
      ${EndIf}
      System::Free $2
      ${IUnknown::Release} $1 ""
    ${EndIf}
    ${IUnknown::Release} $0 ""
  ${EndIf}
  Push $3
!macroend

; Takes a shortcut off the Start menu's and the taskbar's pins.
!macro UnpinShortcut SHORTCUT
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_StartMenuPin} ${IID_IStartMenuPinnedList} r0 ""
  ${If} $0 P<> 0
    System::Call 'SHELL32::SHCreateItemFromParsingName(ws, p0, g "${IID_IShellItem}", *p0r1)' "${SHORTCUT}"
    ${If} $1 P<> 0
      ${IStartMenuPinnedList::RemoveFromList} $0 '(r1)'
      ${IUnknown::Release} $1 ""
    ${EndIf}
    ${IUnknown::Release} $0 ""
  ${EndIf}
!macroend

; Clears the jump list and recent items kept under the AUMID.
!macro DeleteAppUserModelId
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_DestinationList} ${IID_ICustomDestinationList} r1 ""
  ${If} $1 P<> 0
    ${ICustomDestinationList::DeleteList} $1 '("${BUNDLEID}")'
    ${IUnknown::Release} $1 ""
  ${EndIf}
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ApplicationDestinations} ${IID_IApplicationDestinations} r1 ""
  ${If} $1 P<> 0
    ${IApplicationDestinations::SetAppID} $1 '("${BUNDLEID}")i.r0'
    ${If} $0 >= 0
      ${IApplicationDestinations::RemoveAllDestinations} $1 ''
    ${EndIf}
    ${IUnknown::Release} $1 ""
  ${EndIf}
!macroend

; Splits $CMDLINE at the updater's " /ARGS" (a word of its own, after the
; program's name): $CmdHead, the part before it, where the setup's own
; options are read, so that nothing in the tail is taken for one; and
; $RestartArgs, everything after it as it stands, Windows' quoting and all,
; for Luna's restart. ${GetOptions} would take the quotes off a tail that
; opens and ends with a quoted argument. No " /ARGS": all of it is the head.
Function SplitCmdLine
  Push $0
  Push $1
  Push $2
  Push $3
  StrCpy $CmdHead $CMDLINE
  StrCpy $RestartArgs ""
  StrLen $1 $CMDLINE
  ; Past the program's name: to its closing quote, or from the start when
  ; it is not quoted, as a name without a space needs not be.
  StrCpy $0 0
  StrCpy $2 $CMDLINE 1
  ${If} $2 == '"'
    StrCpy $0 1
    ${Do}
      ${If} $0 >= $1
        ${ExitDo}
      ${EndIf}
      StrCpy $2 $CMDLINE 1 $0
      IntOp $0 $0 + 1
      ${If} $2 == '"'
        ${ExitDo}
      ${EndIf}
    ${Loop}
  ${EndIf}
  ${Do}
    ${If} $0 >= $1
      ${ExitDo}
    ${EndIf}
    StrCpy $2 $CMDLINE 6 $0
    ${If} $2 == " /ARGS"
      IntOp $3 $0 + 6
      StrCpy $2 $CMDLINE 1 $3
      ${If} $2 == " "
      ${OrIf} $2 == ""
        StrCpy $CmdHead $CMDLINE $0
        IntOp $3 $3 + 1
        StrCpy $RestartArgs $CMDLINE "" $3
        ${ExitDo}
      ${EndIf}
    ${EndIf}
    IntOp $0 $0 + 1
  ${Loop}
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

Function .onInit
  Call SplitCmdLine
  ; NSIS copies the command line into a string of NSIS_MAX_STRLEN units, its
  ; NUL among them, and cuts the rest: one that fills it may have lost the
  ; end of its /ARGS tail.
  StrCpy $ArgsCut 0
  StrLen $0 $CMDLINE
  IntOp $1 ${NSIS_MAX_STRLEN} - 1
  ${If} $0 >= $1
    StrCpy $ArgsCut 1
    StrCpy $RestartArgs ""
  ${EndIf}
  ${GetOptions} $CmdHead "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}
  ${GetOptions} $CmdHead "/NS" $NoShortcutMode
  ${IfNot} ${Errors}
    StrCpy $NoShortcutMode 1
  ${EndIf}
  ${GetOptions} $CmdHead "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
  !insertmacro SetContext
  ; No /D=: where the last install went, else the per-user default.
  ${If} $INSTDIR == "${PLACEHOLDER_INSTALL_DIR}"
    StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}"
    ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
    ${If} $4 != ""
      StrCpy $INSTDIR $4
    ${EndIf}
  ${EndIf}
FunctionEnd

; An update (/UPDATE, the updater's) never puts an older Luna over the one
; installed here. The updater compares the release with the Luna that runs
; it, which may be an older copy elsewhere (a portable one) than the Luna
; this setup replaces; so the installed one is read here, before anything is
; changed, from what the install folder holds (`LunaInstalledVersion`), and
; compared with this setup's by SemVer precedence (version.nsh): an older
; one is refused, the same one (a repair) goes ahead. Where only the
; installed numbers could be told, a pre-release of the same numbers is
; refused too: what is installed may be their release. Nothing installed,
; or nothing that reads: nothing refused.
Function LunaRefuseDowngrade
  Push $0
  Push $R0
  Push $R1
  Push $R2
  Push $R3
  Call LunaInstalledVersion
  StrCpy $R3 $R0
  ${If} $R3 == ""
    Goto done
  ${EndIf}
  ${If} $R1 = 1
    StrCpy $R1 "${VERSION}"
    Call LunaSemverCompare
    ${If} $R2 = 1
      Goto refuse
    ${EndIf}
    Goto done
  ${EndIf}
  StrCpy $R0 "${VERSION}"
  Call LunaSemverParts
  ${VersionCompare} $R3 $R0 $0
  ${If} $0 == 1
    Goto refuse
  ${EndIf}
  ${If} $0 == 0
  ${AndIf} $R1 != ""
    Goto refuse
  ${EndIf}
  Goto done
  refuse:
  StrCpy $R9 "A newer ${PRODUCTNAME} ($R3) is installed in $INSTDIR than this update (${VERSION}), which would replace it with an older one: the update was not installed."
  !insertmacro LunaRefuse
  done:
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $R0
  Pop $0
FunctionEnd

; One setup at a time from its check of what is installed through the copy
; and the registry's record of it: two at once could each pass the check
; before either copied. A mutex in the session's namespace, waited on; one
; whose setup ended without letting go (a refusal, a crash) is ours at once
; (WAIT_ABANDONED). A setup still holding it after ten minutes, or a mutex
; this setup may not open (an elevated setup's), refuses this one.
Var LunaSetupMutex
Function LunaOneSetupAtATime
  Push $0
  Push $1
  System::Call 'kernel32::CreateMutexW(p 0, i 0, w "Local\${BUNDLEID}-setup") p .r0 ?e'
  Pop $1
  ${If} $0 = 0
    StrCpy $R9 "Another ${PRODUCTNAME} setup is running, and this one cannot wait for it (error $1): run it again once the other is done."
    !insertmacro LunaRefuse
  ${EndIf}
  System::Call 'kernel32::WaitForSingleObject(p r0, i 600000) i .r1'
  ; WAIT_OBJECT_0 or WAIT_ABANDONED.
  ${If} $1 <> 0
  ${AndIf} $1 <> 0x80
    System::Call 'kernel32::CloseHandle(p r0)'
    StrCpy $R9 "Another ${PRODUCTNAME} setup has been running for ten minutes: run this one again once it is done."
    !insertmacro LunaRefuse
  ${EndIf}
  StrCpy $LunaSetupMutex $0
  Pop $1
  Pop $0
FunctionEnd

Function LunaNextSetup
  System::Call 'kernel32::ReleaseMutex(p $LunaSetupMutex)'
  System::Call 'kernel32::CloseHandle(p $LunaSetupMutex)'
  StrCpy $LunaSetupMutex 0
FunctionEnd

Section Install
  SetOutPath $INSTDIR
  ; Before anything is changed, a running Luna stopped included, and one
  ; setup at a time from here through the registry's record.
  Call LunaOneSetupAtATime
  ${If} $UpdateMode = 1
    Call LunaRefuseDowngrade
  ${EndIf}
  !insertmacro NSIS_HOOK_PREINSTALL
  Call LunaCheckIfRunning

  File "/oname=${MAINBINARYNAME}.exe" "${MAINBINARYSRCPATH}"
  File "/oname=THIRD-PARTY-NOTICES.txt" "${NOTICES}"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  ; Where the next install and update go.
  WriteRegStr SHCTX "${MANUPRODUCTKEY}" "" $INSTDIR

  ; An exe of another name a former install left goes.
  ReadRegStr $OldMainBinaryName SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${If} $OldMainBinaryName != ""
  ${AndIf} $OldMainBinaryName != "${MAINBINARYNAME}.exe"
    Delete "$INSTDIR\$OldMainBinaryName"
  ${EndIf}
  WriteRegStr SHCTX "${UNINSTKEY}" "MainBinaryName" "${MAINBINARYNAME}.exe"

  ; Installed apps, Settings.
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoModify" "1"
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRepair" "1"
  ; What this installed, in KB: not the folder, which is the data folder too.
  ${GetSize} "$INSTDIR" "/M=${MAINBINARYNAME}.exe /S=0K /G=0" $0 $1 $2
  ${GetSize} "$INSTDIR" "/M=uninstall.exe /S=0K /G=0" $1 $2 $3
  IntOp $0 $0 + $1
  ${GetSize} "$INSTDIR" "/M=THIRD-PARTY-NOTICES.txt /S=0K /G=0" $1 $2 $3
  IntOp $0 $0 + $1
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD SHCTX "${UNINSTKEY}" "EstimatedSize" "$0"
  ; What is installed here and what the registry says of it agree now: the
  ; next setup may check it.
  Call LunaNextSetup

  Call CreateStartMenuShortcut
  ; The finish page, which offers the desktop one, is skipped in these.
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    Call CreateDesktopShortcut
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTINSTALL
    !insertmacro NSIS_HOOK_POSTINSTALL
  !endif

  ${If} $PassiveMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

; /R, with /P or /S (the updater's): Luna starts again, with what /ARGS
; carries, the arguments the updating Luna ran with, as they stand.
Function .onInstSuccess
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${GetOptions} $CmdHead "/R" $R0
    ${IfNot} ${Errors}
      ${If} $ArgsCut = 1
        DetailPrint "$(argsCut)"
      ${EndIf}
      StrCpy $RunArgs $RestartArgs
      Call LunaRunAsUser
    ${EndIf}
  ${EndIf}
FunctionEnd

; The finish page's Run box.
Function RunMainBinary
  StrCpy $RunArgs ""
  Call LunaRunAsUser
FunctionEnd

; The shell objects LunaRunAsUser goes through (shldisp.h, shobjidl.h,
; servprov.h), and the values it passes them.
!define LUNA_CLSID_ShellWindows "{9BA05972-F6A8-11CF-A442-00A0C90A8F39}"
!define LUNA_IID_IShellWindows "{85CB6900-4D95-11CF-960C-0080C7F4EE85}"
!define LUNA_IID_IServiceProvider "{6D5140C1-7436-11CE-8034-00AA006009FA}"
!define LUNA_SID_STopLevelBrowser "{4C96BE40-915C-11CF-99D3-00AA004AE837}"
!define LUNA_IID_IShellBrowser "{000214E2-0000-0000-C000-000000000046}"
!define LUNA_IID_IDispatch "{00020400-0000-0000-C000-000000000046}"
!define LUNA_IID_IShellFolderViewDual "{E7A1AF80-4D96-11CF-960C-0080C7F4EE85}"
!define LUNA_IID_IShellDispatch2 "{A4C6892C-3BA9-11D2-9DEA-00C04FB16162}"
!define LUNA_CLSCTX_LOCAL_SERVER 4
!define LUNA_SWC_DESKTOP 8
!define LUNA_SWFO_NEEDDISPATCH 1
!define LUNA_SVGIO_BACKGROUND 0
!define LUNA_VT_BSTR 8
!define LUNA_VT_I4 3
!define LUNA_SW_SHOWNORMAL 1

; Pushes whether the process `pid` (empty: this setup) runs elevated, by
; its token's TokenElevation: 0 no, 1 yes, empty when that cannot be read.
; An administrator's token with UAC off counts as elevated.
Function LunaElevation
  Exch $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  StrCpy $5 ""
  ${If} $0 == ""
    System::Call 'kernel32::GetCurrentProcess() p .r1'
  ${Else}
    ; PROCESS_QUERY_LIMITED_INFORMATION
    System::Call 'kernel32::OpenProcess(i 0x1000, i 0, i r0) p .r1'
  ${EndIf}
  ${If} $1 P<> 0
    ; TOKEN_QUERY
    System::Call 'advapi32::OpenProcessToken(p r1, i 8, *p .r2) i .r3'
    ${If} $3 <> 0
      ; TokenElevation (20): a TOKEN_ELEVATION, one DWORD.
      System::Call 'advapi32::GetTokenInformation(p r2, i 20, *i .r4, i 4, *i .r6) i .r3'
      ${If} $3 <> 0
        ${If} $4 = 0
          StrCpy $5 0
        ${Else}
          StrCpy $5 1
        ${EndIf}
      ${EndIf}
      System::Call 'kernel32::CloseHandle(p r2)'
    ${EndIf}
    ; Not this process's own pseudo-handle.
    ${If} $0 != ""
      System::Call 'kernel32::CloseHandle(p r1)'
    ${EndIf}
  ${EndIf}
  StrCpy $0 $5
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Exch $0
FunctionEnd

; Starts "$INSTDIR\luna.exe" $RunArgs as the desktop's own user, never
; elevated, with the environment the shell has rather than this setup's,
; which is the updating Luna's: what Tauri's template does through
; nsis_tauri_utils::RunAsUser. The desktop's shell (explorer) is asked to do
; the start itself: IShellWindows finds its desktop window, whose view
; hands out the shell's IShellDispatch2, and ShellExecute there runs in
; explorer, as explorer runs (Raymond Chen, "How can I launch an
; unelevated process from my elevated process and vice versa?", The Old
; New Thing, 2013-11-18). Each COM call goes through its vtable slot, on
; the 32-bit stub: ShellExecute's four VARIANTs are passed by value, 16
; bytes each, as four dwords (vt and its reserved words, then the value).
; The shell is asked only when its own token is not elevated
; (LunaElevation on the process of the desktop window it found): an
; explorer run elevated, as with UAC off, would start Luna elevated too.
; Otherwise, or with no shell to ask (no explorer, a setup run under
; another desktop): started directly when this setup's token is not
; elevated, since it then runs as the desktop's user anyway. When the
; setup's is elevated, or either cannot be read, Luna is not started at
; all, and the details say so.
Function LunaRunAsUser
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7
  Push $8
  Push $9
  Push $R1
  Push $R2
  Push $R3
  Push $R4
  Push $R5
  Push $R6
  Push $R7
  Push $R8
  ; Started or not.
  StrCpy $9 0
  ; S_OK, or S_FALSE where this thread already had COM: either is undone.
  System::Call 'ole32::CoInitializeEx(p 0, i 2) i .r0'
  StrCpy $R8 $0
  System::Call 'ole32::CoCreateInstance(g "${LUNA_CLSID_ShellWindows}", p 0, i ${LUNA_CLSCTX_LOCAL_SERVER}, g "${LUNA_IID_IShellWindows}", *p .r1) i .r0'
  ${If} $0 = 0
    ; An empty VARIANT (VT_EMPTY, all zero) for both locations.
    System::Alloc 16
    Pop $2
    StrCpy $4 0
    ; IShellWindows::FindWindowSW, slot 15.
    System::Call '$1->15(p r2, p r2, i ${LUNA_SWC_DESKTOP}, *i .r3, i ${LUNA_SWFO_NEEDDISPATCH}, *p .r4) i .r0'
    System::Free $2
    ${If} $0 = 0
    ${AndIf} $4 P<> 0
      ; The shell's own token: it starts Luna as it runs. 0 is no window's
      ; process, which LunaElevation cannot read.
      StrCpy $R7 0
      System::Call 'user32::GetWindowThreadProcessId(p r3, *i .R7) i .r0'
      ${If} $R7 = 0
        StrCpy $R7 ""
      ${Else}
        Push $R7
        Call LunaElevation
        Pop $R7
      ${EndIf}
      ${If} $R7 == "0"
        System::Call '$4->0(g "${LUNA_IID_IServiceProvider}", *p .r5) i .r0'
      ${Else}
        StrCpy $0 1
      ${EndIf}
      ${If} $0 = 0
        ; IServiceProvider::QueryService, slot 3.
        System::Call '$5->3(g "${LUNA_SID_STopLevelBrowser}", g "${LUNA_IID_IShellBrowser}", *p .r6) i .r0'
        ${If} $0 = 0
          ; IShellBrowser::QueryActiveShellView, slot 15.
          System::Call '$6->15(*p .r7) i .r0'
          ${If} $0 = 0
            ; IShellView::GetItemObject, slot 15.
            System::Call '$7->15(i ${LUNA_SVGIO_BACKGROUND}, g "${LUNA_IID_IDispatch}", *p .r8) i .r0'
            ${If} $0 = 0
              System::Call '$8->0(g "${LUNA_IID_IShellFolderViewDual}", *p .R1) i .r0'
              ${If} $0 = 0
                ; IShellFolderViewDual::get_Application, slot 7.
                System::Call '$R1->7(*p .R2) i .r0'
                ${If} $0 = 0
                  System::Call '$R2->0(g "${LUNA_IID_IShellDispatch2}", *p .R3) i .r0'
                  ${If} $0 = 0
                    ; Through registers, not spelled into the call: a
                    ; path or an argument may hold what the call's syntax uses.
                    StrCpy $R7 "$INSTDIR\${MAINBINARYNAME}.exe"
                    System::Call 'oleaut32::SysAllocString(w R7) p .R4'
                    StrCpy $R7 $RunArgs
                    System::Call 'oleaut32::SysAllocString(w R7) p .R5'
                    StrCpy $R7 $INSTDIR
                    System::Call 'oleaut32::SysAllocString(w R7) p .R6'
                    StrCpy $R7 "open"
                    System::Call 'oleaut32::SysAllocString(w R7) p .r3'
                    ; IShellDispatch2::ShellExecute, slot 31: the file, then
                    ; the arguments, the folder and the verb as VT_BSTR, the
                    ; window's show state as VT_I4.
                    System::Call '$R3->31(p R4, i ${LUNA_VT_BSTR}, i 0, p R5, i 0, i ${LUNA_VT_BSTR}, i 0, p R6, i 0, i ${LUNA_VT_BSTR}, i 0, p r3, i 0, i ${LUNA_VT_I4}, i 0, i ${LUNA_SW_SHOWNORMAL}, i 0) i .r0'
                    ${If} $0 = 0
                      StrCpy $9 1
                    ${EndIf}
                    System::Call 'oleaut32::SysFreeString(p R4)'
                    System::Call 'oleaut32::SysFreeString(p R5)'
                    System::Call 'oleaut32::SysFreeString(p R6)'
                    System::Call 'oleaut32::SysFreeString(p r3)'
                    System::Call '$R3->2()'
                  ${EndIf}
                  System::Call '$R2->2()'
                ${EndIf}
                System::Call '$R1->2()'
              ${EndIf}
              System::Call '$8->2()'
            ${EndIf}
            System::Call '$7->2()'
          ${EndIf}
          System::Call '$6->2()'
        ${EndIf}
        System::Call '$5->2()'
      ${EndIf}
      System::Call '$4->2()'
    ${EndIf}
    System::Call '$1->2()'
  ${EndIf}
  ${If} $R8 >= 0
    System::Call 'ole32::CoUninitialize()'
  ${EndIf}
  ${If} $9 = 0
    Push ""
    Call LunaElevation
    Pop $0
    ; The command line Exec takes is one NSIS string: one that fills it
    ; may have been cut, and Luna then starts without the arguments.
    StrCpy $1 '"$INSTDIR\${MAINBINARYNAME}.exe" $RunArgs'
    StrLen $2 $1
    IntOp $3 ${NSIS_MAX_STRLEN} - 1
    ${If} $2 >= $3
      DetailPrint "$(argsCut)"
      StrCpy $1 '"$INSTDIR\${MAINBINARYNAME}.exe"'
    ${EndIf}
    ${If} $0 == "0"
      Exec $1
    ${Else}
      DetailPrint "$(notRestarted)"
    ${EndIf}
  ${EndIf}
  Pop $R8
  Pop $R7
  Pop $R6
  Pop $R5
  Pop $R4
  Pop $R3
  Pop $R2
  Pop $R1
  Pop $9
  Pop $8
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

Function SkipIfPassive
  ${IfThen} $PassiveMode = 1 ${|} Abort ${|}
FunctionEnd

; Not on an update (the one there stays, pins and all), nor with /NS.
Function CreateStartMenuShortcut
  ${If} $UpdateMode = 1
  ${OrIf} $NoShortcutMode = 1
    Return
  ${EndIf}
  CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
FunctionEnd

Function CreateDesktopShortcut
  ${If} $UpdateMode = 1
  ${OrIf} $NoShortcutMode = 1
    Return
  ${EndIf}
  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
FunctionEnd

Function un.onInit
  !insertmacro SetContext
  !insertmacro MUI_UNGETLANGUAGE
  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
FunctionEnd

Function un.SkipIfPassive
  ${IfThen} $PassiveMode = 1 ${|} Abort ${|}
FunctionEnd

; The confirmation page's box for the app data, made by hand: MUI's page is
; no nsDialogs one. Sized for the window's DPI.
Function un.ConfirmShow
  FindWindow $1 "#32770" "" $HWNDPARENT
  System::Call "user32::GetDpiForWindow(p r1) i .r2"
  IntOp $4 0 * $2
  IntOp $5 100 * $2
  IntOp $6 400 * $2
  IntOp $7 25 * $2
  IntOp $4 $4 / 96
  IntOp $5 $5 / 96
  IntOp $6 $6 / 96
  IntOp $7 $7 / 96
  System::Call 'user32::CreateWindowEx(i ${__NSD_CheckBox_EXSTYLE}, w "${__NSD_CheckBox_CLASS}", w "$(deleteAppData)", i ${__NSD_CheckBox_STYLE}, i r4, i r5, i r6, i r7, p r1, i0, i0, i0) i .s'
  Pop $DeleteAppDataCheckbox
  SendMessage $HWNDPARENT ${WM_GETFONT} 0 0 $1
  SendMessage $DeleteAppDataCheckbox ${WM_SETFONT} $1 1
FunctionEnd

Function un.ConfirmLeave
  SendMessage $DeleteAppDataCheckbox ${BM_GETCHECK} 0 0 $DeleteAppDataCheckboxState
FunctionEnd

Section Uninstall
  !insertmacro NSIS_HOOK_PREUNINSTALL
  Call un.LunaCheckIfRunning

  Delete "$INSTDIR\${MAINBINARYNAME}.exe"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.txt"
  Delete "$INSTDIR\uninstall.exe"
  ; Only if empty: it is the data folder too.
  RMDir "$INSTDIR"

  ; An update keeps the shortcuts, their pins and the AUMID's lists.
  ${If} $UpdateMode <> 1
    !insertmacro DeleteAppUserModelId
    !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}
    !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}

  DeleteRegKey HKCU "${UNINSTKEY}"
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
  ${EndIf}

  ; The box ticked, and not on an update: the install location and the
  ; language kept, and what a WebView once kept under the identifier.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"
    RMDir /r "$APPDATA\${BUNDLEID}"
    RMDir /r "$LOCALAPPDATA\${BUNDLEID}"
  ${EndIf}

  !insertmacro NSIS_HOOK_POSTUNINSTALL

  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd
