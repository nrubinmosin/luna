; What Luna is installed in $INSTDIR, and SemVer 2.0.0 precedence, for the
; installer's refusal to put an older Luna over a newer one (installer.nsi
; `LunaRefuseDowngrade`). Plain NSIS and the System plug-in, in a file of
; its own so that a test can include it alone. Needs LogicLib.nsh,
; WordFunc.nsh, and MAINBINARYNAME and UNINSTKEY defined.

; $R0, a SemVer version, as its parts: $R0 its numbers (x.y.z), $R1 its
; pre-release ("" for a release). Build metadata (`+...`) is dropped: it has
; no say in precedence.
Function LunaSemverParts
  Push $0
  Push $1
  Push $2
  StrCpy $R1 ""
  ; The metadata first: a pre-release holds no "+", and metadata may hold "-".
  StrLen $0 $R0
  StrCpy $1 0
  ${Do}
    ${IfThen} $1 >= $0 ${|} ${ExitDo} ${|}
    StrCpy $2 $R0 1 $1
    ${If} $2 == "+"
      StrCpy $R0 $R0 $1
      ${ExitDo}
    ${EndIf}
    IntOp $1 $1 + 1
  ${Loop}
  StrLen $0 $R0
  StrCpy $1 0
  ${Do}
    ${IfThen} $1 >= $0 ${|} ${ExitDo} ${|}
    StrCpy $2 $R0 1 $1
    ${If} $2 == "-"
      IntOp $2 $1 + 1
      StrCpy $R1 $R0 "" $2
      StrCpy $R0 $R0 $1
      ${ExitDo}
    ${EndIf}
    IntOp $1 $1 + 1
  ${Loop}
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; $R0, dot-separated identifiers, as its first in $R0 and the rest in $R1
; ("" when there is none).
Function LunaSemverNext
  Push $0
  Push $1
  Push $2
  StrCpy $R1 ""
  StrLen $0 $R0
  StrCpy $1 0
  ${Do}
    ${IfThen} $1 >= $0 ${|} ${ExitDo} ${|}
    StrCpy $2 $R0 1 $1
    ${If} $2 == "."
      IntOp $2 $1 + 1
      StrCpy $R1 $R0 "" $2
      StrCpy $R0 $R0 $1
      ${ExitDo}
    ${EndIf}
    IntOp $1 $1 + 1
  ${Loop}
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; Whether the identifier $R0 is a number, digits alone: $R1 1 or 0.
Function LunaSemverIsNumber
  Push $0
  Push $1
  Push $2
  StrLen $0 $R0
  StrCpy $R1 0
  ${If} $0 > 0
    StrCpy $R1 1
    StrCpy $1 0
    ${Do}
      ${IfThen} $1 >= $0 ${|} ${ExitDo} ${|}
      StrCpy $2 $R0 1 $1
      ; A character other than 0 reads as 0 unless it is a digit 1 to 9.
      ${If} $2 != "0"
        IntOp $2 $2 + 0
        ${If} $2 = 0
          StrCpy $R1 0
          ${ExitDo}
        ${EndIf}
      ${EndIf}
      IntOp $1 $1 + 1
    ${Loop}
  ${EndIf}
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; $R0 against $R1 by their UTF-16 units (ASCII order for an identifier):
; $R2 -1, 0 or 1.
Function LunaOrdinal
  Push $0
  System::Call 'kernel32::CompareStringOrdinal(w R0, i -1, w R1, i -1, i 0) i .r0'
  ; CSTR_LESS_THAN 1, CSTR_EQUAL 2, CSTR_GREATER_THAN 3.
  IntOp $R2 $0 - 2
  Pop $0
FunctionEnd

; Two pre-release identifiers, $R0 against $R1: numbers by their value and
; below names, names in ASCII order. $R2 -1, 0 or 1.
Function LunaSemverIdentifier
  Push $0
  Push $1
  Push $2
  Push $3
  StrCpy $2 $R0
  StrCpy $3 $R1
  Call LunaSemverIsNumber
  StrCpy $0 $R1
  StrCpy $R0 $3
  Call LunaSemverIsNumber
  StrCpy $1 $R1
  StrCpy $R0 $2
  StrCpy $R1 $3
  ${If} $0 = 1
  ${AndIf} $1 = 1
    ; Without leading zeros (SemVer has none) the longer number is the
    ; larger, and two of one length go as their digits do.
    StrLen $0 $2
    StrLen $1 $3
    ${If} $0 > $1
      StrCpy $R2 1
    ${ElseIf} $0 < $1
      StrCpy $R2 -1
    ${Else}
      Call LunaOrdinal
    ${EndIf}
  ${ElseIf} $0 = 1
    StrCpy $R2 -1
  ${ElseIf} $1 = 1
    StrCpy $R2 1
  ${Else}
    Call LunaOrdinal
  ${EndIf}
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; SemVer $R0 against SemVer $R1 by precedence (SemVer 2.0.0, item 11): the
; numbers; at the same numbers a release above its pre-releases; two
; pre-releases by their identifiers in turn (`LunaSemverIdentifier`), the
; one with fewer below where all it has are equal. Build metadata counts
; for nothing. $R2 is 1 when $R0 is newer, -1 when older, 0 when neither.
; $R0 and $R1 are kept.
Function LunaSemverCompare
  Push $0
  Push $1
  Push $2
  Push $3
  Push $R0
  Push $R1
  StrCpy $3 $R1
  Call LunaSemverParts
  StrCpy $0 $R0
  StrCpy $1 $R1
  StrCpy $R0 $3
  Call LunaSemverParts
  StrCpy $2 $R0
  StrCpy $3 $R1
  ; 0 the same, 1 the first newer, 2 the second.
  ${VersionCompare} $0 $2 $R2
  ${If} $R2 == 2
    StrCpy $R2 -1
  ${EndIf}
  ${If} $R2 == 0
    ${Do}
      ${If} $1 == ""
      ${AndIf} $3 == ""
        ${ExitDo}
      ${EndIf}
      ${If} $1 == ""
        ; A release above a pre-release at the outset; past it, fewer
        ; identifiers below more.
        ${If} $0 != ""
          StrCpy $R2 1
        ${Else}
          StrCpy $R2 -1
        ${EndIf}
        ${ExitDo}
      ${EndIf}
      ${If} $3 == ""
        ${If} $0 != ""
          StrCpy $R2 -1
        ${Else}
          StrCpy $R2 1
        ${EndIf}
        ${ExitDo}
      ${EndIf}
      ; Past the outset: $0 no longer marks it.
      StrCpy $0 ""
      StrCpy $R0 $1
      Call LunaSemverNext
      StrCpy $1 $R1
      StrCpy $2 $R0
      StrCpy $R0 $3
      Call LunaSemverNext
      StrCpy $3 $R1
      StrCpy $R1 $R0
      StrCpy $R0 $2
      Call LunaSemverIdentifier
      ${IfThen} $R2 != 0 ${|} ${ExitDo} ${|}
    ${Loop}
  ${EndIf}
  Pop $R1
  Pop $R0
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; The version of the Luna installed in $INSTDIR, read from what that folder
; holds: $R0 the version ("" when nothing installed reads), $R1 1 when it
; is the whole SemVer version and 0 when only its numbers could be told.
; First the exe's own ProductVersion string, which the build writes from
; Cargo.toml's version, pre-release included, taken where its numbers are
; those of the exe's numeric version; else the uninstall key's
; DisplayVersion, where the key's InstallLocation is this folder and its
; numbers are the exe's (a key another install wrote, or one left from an
; older exe, is not this folder's); else the exe's numbers alone. No exe:
; nothing installed here, whatever the key says.
Function LunaInstalledVersion
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7
  Push $8
  StrCpy $5 "$INSTDIR\${MAINBINARYNAME}.exe"
  StrCpy $R0 ""
  StrCpy $R1 0
  ; The numbers, x.y.z.
  StrCpy $6 ""
  ClearErrors
  GetDLLVersion $5 $0 $1
  ${IfNot} ${Errors}
    IntOp $2 $0 >> 16
    IntOp $2 $2 & 0xFFFF
    IntOp $3 $0 & 0xFFFF
    IntOp $4 $1 >> 16
    IntOp $4 $4 & 0xFFFF
    StrCpy $6 "$2.$3.$4"
  ${EndIf}
  ${If} $6 == ""
    Goto done
  ${EndIf}
  ; Its ProductVersion string, in the language and code page its version
  ; resource lists first.
  StrCpy $7 ""
  System::Call 'version::GetFileVersionInfoSizeW(w r5, p 0) i .r0'
  ${If} $0 > 0
    System::Alloc $0
    Pop $8
    System::Call 'version::GetFileVersionInfoW(w r5, i 0, i r0, p r8) i .r1'
    ${If} $1 <> 0
      System::Call 'version::VerQueryValueW(p r8, w "\VarFileInfo\Translation", *p .r2, *i .r3) i .r1'
      ${If} $1 <> 0
      ${AndIf} $3 >= 4
        System::Call '*$2(&i2 .r3, &i2 .r4)'
        IntOp $3 $3 & 0xFFFF
        IntOp $4 $4 & 0xFFFF
        IntFmt $3 "%04x" $3
        IntFmt $4 "%04x" $4
        System::Call 'version::VerQueryValueW(p r8, w "\StringFileInfo\$3$4\ProductVersion", *p .r2, *i .r3) i .r1'
        ${If} $1 <> 0
        ${AndIf} $3 > 0
        ${AndIf} $3 < ${NSIS_MAX_STRLEN}
          System::Call '*$2(&w$3 .r7)'
        ${EndIf}
      ${EndIf}
    ${EndIf}
    System::Free $8
  ${EndIf}
  ${If} $7 != ""
    StrCpy $R0 $7
    Call LunaSemverParts
    ${VersionCompare} $R0 $6 $0
    ${If} $0 == 0
      StrCpy $R0 $7
      StrCpy $R1 1
      Goto done
    ${EndIf}
  ${EndIf}
  ReadRegStr $7 SHCTX "${UNINSTKEY}" "InstallLocation"
  ReadRegStr $8 SHCTX "${UNINSTKEY}" "DisplayVersion"
  ${If} $8 != ""
  ${AndIf} $7 == "$\"$INSTDIR$\""
    StrCpy $R0 $8
    Call LunaSemverParts
    ${VersionCompare} $R0 $6 $0
    ${If} $0 == 0
      StrCpy $R0 $8
      StrCpy $R1 1
      Goto done
    ${EndIf}
  ${EndIf}
  StrCpy $R0 $6
  StrCpy $R1 0
  done:
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

