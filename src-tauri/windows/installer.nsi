; Luna's installer, made without Tauri's bundler (scripts/build-windows.ps1
; -Installer -Native). It installs as the installer Tauri's NSIS template
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
;   VERSION            x.y.z
;   MAINBINARYSRCPATH  the luna.exe to install
;   NOTICES            THIRD-PARTY-NOTICES.txt, installed beside it
;   INSTALLERICON      icons/icon.ico
;   OUTFILE            the setup to write
;
; Left out from Tauri's: the WebView2 bootstrap (no WebView any more), the
; page offering to uninstall a previous version first (an install always
; goes in place, as an update does), WiX migration (Luna never shipped an
; MSI), file associations and deep links (none), and its helper DLL: the
; version compare, de-elevated start and string replace it did are
; plain NSIS here.

Unicode true
ManifestDPIAware true
ManifestDPIAwareness PerMonitorV2
SetCompressor /SOLID lzma

!ifndef VERSION | MAINBINARYSRCPATH | NOTICES | INSTALLERICON | OUTFILE
  !error "pass VERSION, MAINBINARYSRCPATH, NOTICES, INSTALLERICON and OUTFILE"
!endif

!include MUI2.nsh
!include FileFunc.nsh
!include LogicLib.nsh
!include nsDialogs.nsh
!include x64.nsh
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "Win\RestartManager.nsh"

; What Tauri's bundler made of tauri.conf.json: productName, identifier,
; mainBinaryName, and the publisher, which it takes from the identifier's
; second part when none is configured. An installed 0.19 is found by them.
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

!include "hooks.nsh"

Name "${PRODUCTNAME}"
BrandingText " "
OutFile "${OUTFILE}"
; Not the install folder: .onInit tells a /D= from this and picks the folder.
!define PLACEHOLDER_INSTALL_DIR "placeholder\${PRODUCTNAME}"
InstallDir "${PLACEHOLDER_INSTALL_DIR}"
; Per user, as tauri.conf.json's default installMode, currentUser, has it.
RequestExecutionLevel user

VIProductVersion "${VERSION}.0"
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

Function .onInit
  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/NS" $NoShortcutMode
  ${IfNot} ${Errors}
    StrCpy $NoShortcutMode 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
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

Section Install
  SetOutPath $INSTDIR
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
; carries, the arguments the updating Luna ran with.
Function .onInstSuccess
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${GetOptions} $CMDLINE "/R" $R0
    ${IfNot} ${Errors}
      ${GetOptions} $CMDLINE "/ARGS" $R0
      Exec '"$INSTDIR\${MAINBINARYNAME}.exe" $R0'
    ${EndIf}
  ${EndIf}
FunctionEnd

; The finish page's Run box. The setup runs as the user who started it, a
; per-user install not asking for more, so Luna starts as that user too.
Function RunMainBinary
  Exec '"$INSTDIR\${MAINBINARYNAME}.exe"'
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
