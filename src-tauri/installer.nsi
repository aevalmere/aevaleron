; Aevaleron's installer, built by makensis straight from this file (rust.yml,
; release.yml); there is no tauri-bundler and no WebView2 any more. It does
; what the Tauri installer did, under the same names, so it upgrades a Tauri
; install in place and Apps & features keeps one entry:
;
; - per user, in %LOCALAPPDATA%\Aevaleron (or where the last install went,
;   HKCU\Software\darkelights\Aevaleron), no admin prompt;
; - Aevaleron.exe (cargo builds aevaleron.exe; it is renamed here) and
;   nothing else: the desktop helper runs in process now, and the desktop\
;   folder an older install kept is removed;
; - the uninstall entry HKCU\...\Uninstall\Aevaleron;
; - Start menu and desktop shortcuts carrying the AUMID the toasts use
;   (notify.rs AUMID);
; - PKG-01, D-03 (B-085): a run without /S is one click: a progress bar, then
;   the app starts. /S is silent; the updater (updater.rs) runs
;   `/S --updated [--force-run]` and --force-run starts the app afterwards;
; - PARITY UPD-15: an AevalBuddy copy (Electron, or a 2.0 Tauri build from
;   before the rename) is uninstalled first; its user data moves on the
;   app's first start (paths.rs);
; - the WebView2 profile the Tauri build kept is deleted, install and
;   uninstall alike. %APPDATA%\aevaleron (settings, key, history) is never
;   touched.
;
;   makensis /DVERSION=1.1.1 [/DVERSIONWITHBUILD=1.1.1.0] [/DSRCEXE=...]
;            [/DOUTFILE=...] src-tauri\installer.nsi
;
; Relative paths are relative to this file's folder (makensis changes into
; it). The output folder has to exist.

Unicode true
ManifestDPIAware true
ManifestDPIAwareness PerMonitorV2
SetCompressor /SOLID lzma

!include MUI2.nsh
!include FileFunc.nsh
!include LogicLib.nsh
!include x64.nsh
!include WinMessages.nsh
!include nsDialogs.nsh
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"

!ifndef VERSION
  !error "pass the version: makensis /DVERSION=1.2.3 installer.nsi"
!endif
; VIProductVersion takes four numbers; CI strips any prerelease part.
!ifndef VERSIONWITHBUILD
  !define VERSIONWITHBUILD "${VERSION}.0"
!endif
!ifndef SRCEXE
  !define SRCEXE "target\release\aevaleron.exe"
!endif
!ifndef OUTFILE
  !define OUTFILE "target\release\bundle\nsis\Aevaleron_${VERSION}_x64-setup.exe"
!endif

!define PRODUCTNAME "Aevaleron"
!define MANUFACTURER "darkelights"
!define MAINBINARY "Aevaleron.exe"
; notify.rs AUMID and the old bundle identifier.
!define AUMID "com.aevaleron.app"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
!define MANUKEY "Software\${MANUFACTURER}"
!define MANUPRODUCTKEY "${MANUKEY}\${PRODUCTNAME}"
!define RUNKEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define APPROVEDKEY "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
!define /ifndef STGM_READWRITE 2

Name "${PRODUCTNAME}"
BrandingText "${PRODUCTNAME} ${VERSION}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\${PRODUCTNAME}"
InstallDirRegKey HKCU "${MANUPRODUCTKEY}" ""

VIProductVersion "${VERSIONWITHBUILD}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
VIAddVersionKey "CompanyName" "${MANUFACTURER}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

!define MUI_ICON "icons\icon.ico"
!define MUI_UNICON "icons\icon.ico"

Var PassiveMode
Var UpdateMode
Var NoShortcutMode
Var OneClick
Var DeleteAppDataCheckbox
Var DeleteAppDataCheckboxState

; ------------------------------------------------------------------ helpers

; Sets the AppUserModelID on a .lnk, so a toast from the app (same AUMID) is
; shown and a click on it reaches the app.
!macro SetLnkAppUserModelId shortcut
  !insertmacro ComHlpr_CreateInProcInstance ${CLSID_ShellLink} ${IID_IShellLink} r0 ""
  ${If} $0 P<> 0
    ${IUnknown::QueryInterface} $0 '("${IID_IPersistFile}",.r1)'
    ${If} $1 P<> 0
      ${IPersistFile::Load} $1 '("${shortcut}",${STGM_READWRITE})'
      ${IUnknown::QueryInterface} $0 '("${IID_IPropertyStore}",.r2)'
      ${If} $2 P<> 0
        System::Call 'Oleaut32::SysAllocString(w "${AUMID}")p.r3'
        System::Call '*${SYSSTRUCT_PROPERTYKEY}(${PKEY_AppUserModel_ID})p.r4'
        System::Call '*${SYSSTRUCT_PROPVARIANT}(${VT_BSTR},,&i${NSIS_PTR_SIZE} $3)p.r5'
        ${IPropertyStore::SetValue} $2 '($4,$5)'
        ${IPropertyStore::Commit} $2 ""
        System::Call 'Oleaut32::SysFreeString(p r3)'
        System::Free $4
        System::Free $5
        ${IUnknown::Release} $2 ""
        ${IPersistFile::Save} $1 '("${shortcut}",1)'
      ${EndIf}
      ${IUnknown::Release} $1 ""
    ${EndIf}
    ${IUnknown::Release} $0 ""
  ${EndIf}
!macroend

; Closes a running copy so its exe can be replaced, then waits (up to 10 s)
; until the file opens for writing. Only the app itself is killed, not its
; tree: the updater starts this installer from the app.
!macro CloseRunningApp
  nsExec::Exec 'taskkill /F /IM ${MAINBINARY}'
  Pop $0
  ${If} ${FileExists} "$INSTDIR\${MAINBINARY}"
    StrCpy $1 0
    ${Do}
      ClearErrors
      FileOpen $0 "$INSTDIR\${MAINBINARY}" a
      ${IfNot} ${Errors}
        FileClose $0
        ${Break}
      ${EndIf}
      IntOp $1 $1 + 1
      ${If} $1 > 40
        ${Break}
      ${EndIf}
      Sleep 250
    ${Loop}
  ${EndIf}
!macroend

; The Tauri build's WebView2 profile: paths.rs webview_data_dir
; (%LOCALAPPDATA%\aevaleron\WebView2), and Tauri's own default folder.
!macro RemoveWebViewData
  RMDir /r "$LOCALAPPDATA\aevaleron\WebView2"
  RMDir /r "$LOCALAPPDATA\${AUMID}\EBWebView"
  RMDir "$LOCALAPPDATA\${AUMID}"
!macroend

; PARITY UPD-15. The app was AevalBuddy before 2.0.
;
; An Electron copy installed by electron-builder (per user, under
; %LOCALAPPDATA%\Programs\aevalbuddy) is uninstalled silently first. Its
; uninstaller keeps %APPDATA%\aevalbuddy (deleteAppDataOnUninstall: false);
; the app moves that folder to %APPDATA%\aevaleron on its first start
; (paths.rs), so the settings, key, history and shortcuts carry over. The Run
; key is rewritten by the new app at boot (autostart.rs).
;
; A 2.0 test build from before the rename (Tauri, in %LOCALAPPDATA%\AevalBuddy
; with uninstall.exe beside AevalBuddy.exe) is removed the same way, with the
; browser profile and update downloads it kept in its install folder.
;
; electron-builder's uninstall entry is "AevalBuddy <version>", and it does
; not always write InstallLocation, so the entry is matched by the first ten
; characters of DisplayName and the folder is taken from UninstallString
; ("<dir>\Uninstall AevalBuddy.exe" /currentuser), falling back to
; InstallLocation.
!macro RemoveAevalBuddy
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  StrCpy $0 0
  aeval_scan:
    EnumRegKey $1 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall" $0
    StrCmp $1 "" aeval_done
    IntOp $0 $0 + 1
    ReadRegStr $2 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\$1" "DisplayName"
    StrCpy $4 $2 10
    StrCmp $4 "AevalBuddy" 0 aeval_scan
    ReadRegStr $3 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\$1" "UninstallString"
    StrCmp $3 "" aeval_location
    ; A quoted path: keep what is between the quotes.
    StrCpy $4 $3 1
    StrCmp $4 '"' 0 aeval_parent
    StrCpy $3 $3 "" 1
    StrCpy $5 0
  aeval_quote:
    StrCpy $4 $3 1 $5
    StrCmp $4 "" aeval_cut
    StrCmp $4 '"' aeval_cut
    IntOp $5 $5 + 1
    Goto aeval_quote
  aeval_cut:
    StrCpy $3 $3 $5
  aeval_parent:
    ${GetParent} "$3" $3
    StrCmp $3 "" aeval_location aeval_check
  aeval_location:
    ReadRegStr $3 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\$1" "InstallLocation"
    StrCmp $3 "" aeval_scan
  aeval_check:
    ; electron-builder names its uninstaller "Uninstall <productName>.exe";
    ; Tauri's own is uninstall.exe.
    IfFileExists "$3\Uninstall AevalBuddy.exe" aeval_electron
    IfFileExists "$3\AevalBuddy.exe" 0 aeval_scan
    IfFileExists "$3\uninstall.exe" 0 aeval_scan
    DetailPrint "Removing the previous AevalBuddy from $3"
    ExecWait '"$3\uninstall.exe" /S _?=$3'
    Delete "$3\uninstall.exe"
    RMDir /r "$3\WebView2"
    RMDir /r "$3\pending"
    RMDir "$3"
    Goto aeval_done
  aeval_electron:
    DetailPrint "Removing the previous AevalBuddy (Electron) from $3"
    ; _?= runs the uninstaller in place and waits for it.
    ExecWait '"$3\Uninstall AevalBuddy.exe" /S /currentuser --updated _?=$3'
    Delete "$3\Uninstall AevalBuddy.exe"
    RMDir "$3"
    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\$1"
  aeval_done:
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
!macroend

; ------------------------------------------------------------------ pages

; B-085: no welcome, license, folder or finish page; the progress bar only.
!insertmacro MUI_PAGE_INSTFILES

; The uninstaller asks once, with the box Tauri's had: "Delete the
; application data" removes the install-location key and the folders named
; after the bundle id. %APPDATA%\aevaleron stays either way.
!define MUI_PAGE_CUSTOMFUNCTION_PRE un.SkipIfPassive
!define MUI_PAGE_CUSTOMFUNCTION_SHOW un.ConfirmShow
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE un.ConfirmLeave
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Function un.SkipIfPassive
  ${IfThen} $PassiveMode = 1 ${|} Abort ${|}
FunctionEnd

Function un.ConfirmShow
  ; $1 inner dialog, $2 its DPI, $3..$6 the box's place and size.
  FindWindow $1 "#32770" "" $HWNDPARENT
  System::Call "user32::GetDpiForWindow(p r1) i .r2"
  ${If} $2 = 0
    StrCpy $2 96
  ${EndIf}
  IntOp $3 0 * $2
  IntOp $4 100 * $2
  IntOp $5 400 * $2
  IntOp $6 25 * $2
  IntOp $3 $3 / 96
  IntOp $4 $4 / 96
  IntOp $5 $5 / 96
  IntOp $6 $6 / 96
  System::Call 'user32::CreateWindowEx(i ${__NSD_CheckBox_EXSTYLE}, w "${__NSD_CheckBox_CLASS}", w "Delete the application data", i ${__NSD_CheckBox_STYLE}, i r3, i r4, i r5, i r6, p r1, i0, i0, i0) p .s'
  Pop $DeleteAppDataCheckbox
  SendMessage $HWNDPARENT ${WM_GETFONT} 0 0 $1
  SendMessage $DeleteAppDataCheckbox ${WM_SETFONT} $1 1
FunctionEnd

Function un.ConfirmLeave
  SendMessage $DeleteAppDataCheckbox ${BM_GETCHECK} 0 0 $DeleteAppDataCheckboxState
FunctionEnd

; ------------------------------------------------------------------ install

Function .onInit
  ${GetParameters} $R0
  StrCpy $PassiveMode 0
  StrCpy $UpdateMode 0
  StrCpy $NoShortcutMode 0
  StrCpy $OneClick 0
  ClearErrors
  ${GetOptions} $R0 "/P" $R1
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/NS" $R1
  ${IfNot} ${Errors}
    StrCpy $NoShortcutMode 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/UPDATE" $R1
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
  ; B-085: an interactive run (no /S, no /P) is one click: the passive path,
  ; then the app starts.
  ${IfNot} ${Silent}
  ${AndIf} $PassiveMode <> 1
    StrCpy $PassiveMode 1
    StrCpy $OneClick 1
  ${EndIf}
FunctionEnd

Section Install
  SetOutPath $INSTDIR

  !insertmacro RemoveAevalBuddy
  !insertmacro CloseRunningApp

  File "/oname=${MAINBINARY}" "${SRCEXE}"

  ; The prebuilt helper DLL older installs kept; the helper is in process now.
  Delete "$INSTDIR\desktop\desktop-*.dll"
  RMDir "$INSTDIR\desktop"

  !insertmacro RemoveWebViewData

  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${MANUPRODUCTKEY}" "" $INSTDIR

  WriteRegStr HKCU "${UNINSTKEY}" "MainBinaryName" "${MAINBINARY}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\${MAINBINARY}$\""
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr HKCU "${UNINSTKEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K /G=1" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINSTKEY}" "EstimatedSize" "$0"

  ; Shortcuts, as Tauri made them: Start menu always, the desktop too since
  ; every run here is passive or silent. Skipped for /UPDATE and /NS.
  ${If} $UpdateMode <> 1
  ${AndIf} $NoShortcutMode <> 1
    CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARY}"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARY}"
    !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
  ${EndIf}

  ; The updater's relaunch.
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "--force-run" $1
  ${IfNot} ${Errors}
    Exec '"$INSTDIR\${MAINBINARY}"'
  ${EndIf}

  SetAutoClose true
SectionEnd

Function .onInstSuccess
  ${If} $OneClick = 1
    Exec '"$INSTDIR\${MAINBINARY}"'
    Return
  ${EndIf}
  ; Tauri's /R: a silent or passive run that asks for a start.
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/R" $1
  ${IfNot} ${Errors}
    Exec '"$INSTDIR\${MAINBINARY}"'
  ${EndIf}
FunctionEnd

; ------------------------------------------------------------------ uninstall

Function un.onInit
  ${GetParameters} $R0
  StrCpy $PassiveMode 0
  StrCpy $UpdateMode 0
  ClearErrors
  ${GetOptions} $R0 "/P" $R1
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/UPDATE" $R1
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
FunctionEnd

Section Uninstall
  !insertmacro CloseRunningApp

  Delete "$INSTDIR\${MAINBINARY}"
  Delete "$INSTDIR\desktop\desktop-*.dll"
  RMDir "$INSTDIR\desktop"
  Delete "$INSTDIR\uninstall.exe"
  !insertmacro RemoveWebViewData
  RMDir "$INSTDIR"

  ${If} $UpdateMode <> 1
    Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ; autostart.rs VALUE_NAME, and the name Tauri's template removed.
    DeleteRegValue HKCU "${RUNKEY}" "${AUMID}"
    DeleteRegValue HKCU "${APPROVEDKEY}" "${AUMID}"
    DeleteRegValue HKCU "${RUNKEY}" "${PRODUCTNAME}"
  ${EndIf}

  DeleteRegKey HKCU "${UNINSTKEY}"

  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    DeleteRegKey HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"
    RMDir /r "$APPDATA\${AUMID}"
    RMDir /r "$LOCALAPPDATA\${AUMID}"
  ${EndIf}

  SetAutoClose true
SectionEnd
