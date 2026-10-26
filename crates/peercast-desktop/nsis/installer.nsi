; PeerCast RT custom NSIS template.
; Based on Tauri's official NSIS template for the 2.12 series and adapted to
; use a full-window nsDialogs welcome/finish experience.
Unicode true
ManifestDPIAware true
; Add in `dpiAwareness` `PerMonitorV2` to manifest for Windows 10 1607+ (note this should not affect lower versions since they should be able to ignore this and pick up `dpiAware` `true` set by `ManifestDPIAware true`)
; Currently undocumented on NSIS's website but is in the Docs folder of source tree, see
; https://github.com/kichik/nsis/blob/5fc0b87b819a9eec006df4967d08e522ddd651c9/Docs/src/attributes.but#L286-L300
; https://github.com/tauri-apps/tauri/pull/10106
ManifestDPIAwareness PerMonitorV2

!if "{{compression}}" == "none"
  SetCompress off
!else
  ; Set the compression algorithm. We default to LZMA.
  SetCompressor /SOLID "{{compression}}"
!endif

; Keep above !include to stay ahead of any plugin command
; see https://github.com/tauri-apps/tauri/pull/15422#discussion_r3289239624
{{#if signed_plugins_path}}
!addplugindir "{{signed_plugins_path}}"
{{/if}}

!include MUI2.nsh
!define MUI_BGCOLOR "F8F6FD"
!define MUI_TEXTCOLOR "241C35"
!define MUI_INSTFILESPAGE_COLORS "514A5E F8F6FD"
!include nsDialogs.nsh
!include WinMessages.nsh
!include FileFunc.nsh
!include x64.nsh
!include WordFunc.nsh
!include "utils.nsh"
!include "FileAssociation.nsh"
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "Win\RestartManager.nsh"
!include "StrFunc.nsh"
${StrCase}
${StrLoc}

!define MUI_CUSTOMFUNCTION_GUIINIT PeerCastGUIInit
!define MUI_CUSTOMFUNCTION_UNGUIINIT un.PeerCastGUIInit

{{#if installer_hooks}}
!include "{{installer_hooks}}"
{{/if}}

!define WEBVIEW2APPGUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

!define MANUFACTURER "{{manufacturer}}"
!define PRODUCTNAME "{{product_name}}"
!define VERSION "{{version}}"
!define VERSIONWITHBUILD "{{version_with_build}}"
!define HOMEPAGE "{{homepage}}"
!define INSTALLMODE "{{install_mode}}"
!define LICENSE "{{license}}"
!define INSTALLERICON "{{installer_icon}}"
!define SIDEBARIMAGE "{{sidebar_image}}"
!define HEADERIMAGE "{{header_image}}"
!define UNINSTALLERICON "{{uninstaller_icon}}"
!define UNINSTALLERHEADERIMAGE "{{uninstaller_header_image}}"
!define MAINBINARYNAME "{{main_binary_name}}"
!define MAINBINARYSRCPATH "{{main_binary_path}}"
!define BUNDLEID "{{bundle_id}}"
!define COPYRIGHT "{{copyright}}"
!define OUTFILE "{{out_file}}"
!define ARCH "{{arch}}"
!define ADDITIONALPLUGINSPATH "{{additional_plugins_path}}"
!define ALLOWDOWNGRADES "{{allow_downgrades}}"
!define DISPLAYLANGUAGESELECTOR "{{display_language_selector}}"
!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"
!define WEBVIEW2INSTALLERARGS "{{webview2_installer_args}}"
!define WEBVIEW2BOOTSTRAPPERPATH "{{webview2_bootstrapper_path}}"
!define WEBVIEW2INSTALLERPATH "{{webview2_installer_path}}"
!define MINIMUMWEBVIEW2VERSION "{{minimum_webview2_version}}"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\PeerCast RT"
!define MANUKEY "Software\4getkun"
!define MANUPRODUCTKEY "${MANUKEY}\PeerCast RT"
!define UNINSTALLERSIGNCOMMAND "{{uninstaller_sign_cmd}}"
!define ESTIMATEDSIZE "{{estimated_size}}"
!define STARTMENUFOLDER "{{start_menu_folder}}"

Var PassiveMode
Var UpdateMode
Var NoShortcutMode
Var WixMode
Var OldMainBinaryName
Var ReadyPathControl
Var ReadyLaunchCheckbox
Var ReadyDesktopCheckbox
Var FinishLaunchCheckbox
Var FinishDesktopCheckbox
Var ReadyLaunch
Var ReadyDesktop
Var ReadyBackNavigation
Var FinishBackNavigation
Var FinishActionsRun
Var InstallSucceeded
Var ReadyDialog
Var FinishDialog
Var InstallerLogoHandle
Var InstallerTitleFont
Var InstallerHeadingFont
Var InstallerBodyFont
Var InstallerSmallFont
Var ReadyExistingMode
Var InstallerContentWidth
Var InstallerCardWidth
Var ProgressLogoHandle
Var ProgressTitleFont
Var ProgressBodyFont
Var ReadyCard
Var FinishCard
Var UninstallCard
Var UninstallLogoHandle
Var UninstallSmallFont
Var UninstallBodyFont

Name "${PRODUCTNAME}"
BrandingText "${PRODUCTNAME} ${VERSION}"
OutFile "${OUTFILE}"

; We don't actually use this value as default install path,
; it's just for nsis to append the product name folder in the directory selector
; https://nsis.sourceforge.io/Reference/InstallDir
!define PLACEHOLDER_INSTALL_DIR "placeholder\${PRODUCTNAME}"
InstallDir "${PLACEHOLDER_INSTALL_DIR}"
AllowRootDirInstall false

VIProductVersion "${VERSIONWITHBUILD}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

# additional plugins
!addplugindir "${ADDITIONALPLUGINSPATH}"

; Uninstaller signing command
!if "${UNINSTALLERSIGNCOMMAND}" != ""
  !uninstfinalize '${UNINSTALLERSIGNCOMMAND}'
!endif

; Handle install mode, `perUser`, `perMachine` or `both`
!if "${INSTALLMODE}" == "perMachine"
  RequestExecutionLevel admin
!endif

!if "${INSTALLMODE}" == "currentUser"
  RequestExecutionLevel user
!endif

!if "${INSTALLMODE}" == "both"
  !define MULTIUSER_MUI
  !define MULTIUSER_INSTALLMODE_INSTDIR "${PRODUCTNAME}"
  !define MULTIUSER_INSTALLMODE_COMMANDLINE
  !if "${ARCH}" == "x64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !else if "${ARCH}" == "arm64"
    !define MULTIUSER_USE_PROGRAMFILES64
  !endif
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_KEY "${UNINSTKEY}"
  !define MULTIUSER_INSTALLMODE_DEFAULT_REGISTRY_VALUENAME "CurrentUser"
  !define MULTIUSER_INSTALLMODEPAGE_SHOWUSERNAME
  !define MULTIUSER_INSTALLMODE_FUNCTION RestorePreviousInstallLocation
  !define MULTIUSER_EXECUTIONLEVEL Highest
  !include MultiUser.nsh
!endif

; Installer icon
!if "${INSTALLERICON}" != ""
  !define MUI_ICON "${INSTALLERICON}"
!endif

; Installer sidebar image
!if "${SIDEBARIMAGE}" != ""
  !define MUI_WELCOMEFINISHPAGE_BITMAP "${SIDEBARIMAGE}"
!endif

; Enable header images for installer and uninstaller pages when either image is configured.
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!else if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!endif

; Installer header image
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_BITMAP "${HEADERIMAGE}"
!endif

; Uninstaller header image
!if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_UNBITMAP "${UNINSTALLERHEADERIMAGE}"
!endif

; Uninstaller icon
!if "${UNINSTALLERICON}" != ""
  !define MUI_UNICON "${UNINSTALLERICON}"
!else if "${INSTALLERICON}" != ""
  !define MUI_UNICON "${INSTALLERICON}"
!endif

; Define registry key to store installer language
!define MUI_LANGDLL_REGISTRY_ROOT "HKCU"
!define MUI_LANGDLL_REGISTRY_KEY "${MANUPRODUCTKEY}"
!define MUI_LANGDLL_REGISTRY_VALUENAME "Installer Language"
!define MUI_LANGDLL_ALWAYSSHOW

; Installer pages, must be ordered as they appear
; The full GPL text is installed with the application and linked from the
; distribution page. Do not precede the branded welcome with MUI's stock
; license page, whose fixed child dialog cannot follow the wide high-DPI UI.

; 2. Install mode (if it is set to `both`)
!if "${INSTALLMODE}" == "both"
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipInstallModePageIfNeeded
  !insertmacro MULTIUSER_PAGE_INSTALLMODE
!endif

; 3. Custom page to ask user if he wants to reinstall/uninstall
;    only if a previous installation was detected
Var ReinstallPageCheck
Page custom PageReinstall PageLeaveReinstall
Function PageReinstall
  ; Uninstall previous WiX installation if exists.
  ;
  ; A WiX installer stores the installation info in registry
  ; using a UUID and so we have to loop through all keys under
  ; `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`
  ; and check if `DisplayName` and `Publisher` keys match ${PRODUCTNAME} and ${MANUFACTURER}
  ;
  ; This has a potential issue that there maybe another installation that matches
  ; our ${PRODUCTNAME} and ${MANUFACTURER} but wasn't installed by our WiX installer,
  ; however, this should be fine since the user will have to confirm the uninstallation
  ; and they can chose to abort it if doesn't make sense.
  StrCpy $0 0
  wix_loop:
    EnumRegKey $1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall" $0
    StrCmp $1 "" wix_loop_done ; Exit loop if there is no more keys to loop on
    IntOp $0 $0 + 1
    ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "DisplayName"
    ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "Publisher"
    StrCmp "$R0$R1" "${PRODUCTNAME}${MANUFACTURER}" 0 wix_loop
    ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "UninstallString"
    ${StrCase} $R1 $R0 "L"
    ${StrLoc} $R0 $R1 "msiexec" ">"
    StrCmp $R0 0 0 wix_loop_done
    StrCpy $WixMode 1
    StrCpy $R6 "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1"
    Goto compare_version
  wix_loop_done:

  ; Check if there is an existing installation, if not, abort the reinstall page
  ReadRegStr $R0 SHCTX "${UNINSTKEY}" ""
  ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
  ${IfThen} "$R0$R1" == "" ${|} Abort ${|}

  ; Compare this installar version with the existing installation
  ; and modify the messages presented to the user accordingly
  compare_version:
  StrCpy $R4 "$(older)"
  ${If} $WixMode = 1
    ReadRegStr $R0 HKLM "$R6" "DisplayVersion"
  ${Else}
    ReadRegStr $R0 SHCTX "${UNINSTKEY}" "DisplayVersion"
  ${EndIf}
  ${IfThen} $R0 == "" ${|} StrCpy $R4 "$(unknown)" ${|}

  nsis_tauri_utils::SemverCompare "${VERSION}" $R0
  Pop $R0
  ; Reinstalling the same version
  ${If} $R0 = 0
    StrCpy $R1 "$(alreadyInstalledLong)"
    StrCpy $R2 "$(addOrReinstall)"
    StrCpy $R3 "$(uninstallApp)"
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(chooseMaintenanceOption)"
  ; Upgrading
  ${ElseIf} $R0 = 1
    StrCpy $R1 "$(olderOrUnknownVersionInstalled)"
    StrCpy $R2 "$(uninstallBeforeInstalling)"
    StrCpy $R3 "$(dontUninstall)"
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(choowHowToInstall)"
  ; Downgrading
  ${ElseIf} $R0 = -1
    StrCpy $R1 "$(newerVersionInstalled)"
    StrCpy $R2 "$(uninstallBeforeInstalling)"
    !if "${ALLOWDOWNGRADES}" == "true"
      StrCpy $R3 "$(dontUninstall)"
    !else
      StrCpy $R3 "$(dontUninstallDowngrade)"
    !endif
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(choowHowToInstall)"
  ${Else}
    Abort
  ${EndIf}

  ; Passive and update invocations must never run the WiX uninstaller.
  ${If} $WixMode = 1
  ${AndIf} $PassiveMode = 1
    StrCpy $WixMode 0
    Abort
  ${EndIf}
  ${If} $WixMode = 1
  ${AndIf} $UpdateMode = 1
    StrCpy $WixMode 0
    Abort
  ${EndIf}
  ${If} $WixMode = 1
  ${AndIf} ${Silent}
    StrCpy $WixMode 0
    Abort
  ${EndIf}

  ; Existing NSIS installs are updated in place. The old maintenance page could
  ; launch the uninstaller and then continue into the installer, which looked
  ; like a second install after removal. Keep that choice only for WiX migration.
  ${If} $WixMode != 1
    ${If} $R0 = 1
      StrCpy $ReadyExistingMode 1
    ${ElseIf} $R0 = 0
      StrCpy $ReadyExistingMode 2
    ${ElseIf} $R0 = -1
      !if "${ALLOWDOWNGRADES}" == "true"
        StrCpy $ReadyExistingMode 3
      !else
        MessageBox MB_OK|MB_ICONEXCLAMATION "$(installerDowngradeBlocked)"
        Quit
      !endif
    ${EndIf}
    Abort
  ${EndIf}

  ; Skip showing the page if passive
  ;
  ; Note that we don't call this earlier at the beginning
  ; of this function because we need to populate some variables
  ; related to current installed version if detected and whether
  ; we are downgrading or not.
  ${If} $PassiveMode = 1
    Call PageLeaveReinstall
  ${Else}
    nsDialogs::Create 1018
    Pop $R4
    ${IfThen} $(^RTL) = 1 ${|} nsDialogs::SetRTL $(^RTL) ${|}

    ${NSD_CreateLabel} 0 0 100% 24u $R1
    Pop $R1

    ${NSD_CreateRadioButton} 30u 50u -30u 8u $R2
    Pop $R2
    ${NSD_OnClick} $R2 PageReinstallUpdateSelection

    ${NSD_CreateRadioButton} 30u 70u -30u 8u $R3
    Pop $R3
    ; Disable this radio button if downgrading and downgrades are disabled
    !if "${ALLOWDOWNGRADES}" == "false"
      ${IfThen} $R0 = -1 ${|} EnableWindow $R3 0 ${|}
    !endif
    ${NSD_OnClick} $R3 PageReinstallUpdateSelection

    ; Check the first radio button if this the first time
    ; we enter this page or if the second button wasn't
    ; selected the last time we were on this page
    ${If} $ReinstallPageCheck <> 2
      SendMessage $R2 ${BM_SETCHECK} ${BST_CHECKED} 0
    ${Else}
      SendMessage $R3 ${BM_SETCHECK} ${BST_CHECKED} 0
    ${EndIf}

    ${NSD_SetFocus} $R2
    nsDialogs::Show
  ${EndIf}
FunctionEnd
Function PageReinstallUpdateSelection
  ${NSD_GetState} $R2 $R1
  ${If} $R1 == ${BST_CHECKED}
    StrCpy $ReinstallPageCheck 1
  ${Else}
    StrCpy $ReinstallPageCheck 2
  ${EndIf}
FunctionEnd
Function PageLeaveReinstall
  ${NSD_GetState} $R2 $R1
  ${If} $R1 == ${BST_CHECKED}
    StrCpy $ReinstallPageCheck 1
  ${Else}
    StrCpy $ReinstallPageCheck 2
  ${EndIf}

  ; If migrating from Wix, always uninstall
  ${If} $WixMode = 1
    Goto reinst_uninstall
  ${EndIf}

  ; In update mode, always proceeds without uninstalling
  ${If} $UpdateMode = 1
    Goto reinst_done
  ${EndIf}

  ; $R0 holds whether same(0)/upgrading(1)/downgrading(-1) version
  ; $R1 holds the radio buttons state:
  ;   1 => first choice was selected
  ;   0 => second choice was selected
  ${If} $R0 = 0 ; Same version, proceed
    ${If} $R1 = 1              ; User chose to add/reinstall
      Goto reinst_done
    ${Else}                    ; User chose to uninstall
      Goto reinst_uninstall
    ${EndIf}
  ${ElseIf} $R0 = 1 ; Upgrading
    ${If} $R1 = 1              ; User chose to uninstall
      Goto reinst_uninstall
    ${Else}
      Goto reinst_done         ; User chose NOT to uninstall
    ${EndIf}
  ${ElseIf} $R0 = -1 ; Downgrading
    ${If} $R1 = 1              ; User chose to uninstall
      Goto reinst_uninstall
    ${Else}
      Goto reinst_done         ; User chose NOT to uninstall
    ${EndIf}
  ${EndIf}

  reinst_uninstall:
    HideWindow
    ClearErrors

    ${If} $WixMode = 1
      ReadRegStr $R1 HKLM "$R6" "UninstallString"
      ExecWait '$R1' $0
    ${Else}
      ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
      ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
      ${IfThen} $UpdateMode = 1 ${|} StrCpy $R1 "$R1 /UPDATE" ${|} ; append /UPDATE
      ${IfThen} $PassiveMode = 1 ${|} StrCpy $R1 "$R1 /P" ${|} ; append /P
      StrCpy $R1 "$R1 _?=$4" ; append uninstall directory
      ExecWait '$R1' $0
    ${EndIf}

    BringToFront

    ${IfThen} ${Errors} ${|} StrCpy $0 2 ${|} ; ExecWait failed, set fake exit code

    ${If} $0 <> 0
    ${OrIf} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
      ; User cancelled wix uninstaller? return to select un/reinstall page
      ${If} $WixMode = 1
      ${AndIf} $0 = 1602
        Abort
      ${EndIf}

      ; User cancelled NSIS uninstaller? return to select un/reinstall page
      ${If} $0 = 1
        Abort
      ${EndIf}

      ; Other errors? show generic error message and return to select un/reinstall page
      MessageBox MB_ICONEXCLAMATION "$(unableToUninstall)"
      Abort
    ${EndIf}
    ; A same-version WiX maintenance choice explicitly labelled Uninstall is
    ; uninstall-only; do not continue into the setup page after it succeeds.
    ${If} $WixMode = 1
    ${AndIf} $R0 = 0
    ${AndIf} $ReinstallPageCheck = 2
      Quit
    ${EndIf}
  reinst_done:
FunctionEnd

; 4. Combined welcome and install-location page. Keep it after maintenance
; choices so those pages cannot replace the user's final setup selections.
Page custom PageReady PageReadyLeave

; 5. Start menu shortcut page
Var AppStartMenuFolder
!if "${STARTMENUFOLDER}" != ""
  ; Keep a predictable Start menu shortcut without showing a stock MUI page
  ; between the branded welcome and install-progress views.
  !define MUI_PAGE_CUSTOMFUNCTION_PRE Skip
  !define MUI_STARTMENUPAGE_DEFAULTFOLDER "${STARTMENUFOLDER}"
!else
  !define MUI_PAGE_CUSTOMFUNCTION_PRE Skip
!endif
!insertmacro MUI_PAGE_STARTMENU Application $AppStartMenuFolder

; 6. Installation page
!define MUI_PAGE_CUSTOMFUNCTION_SHOW PeerCastProgressShow
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE PeerCastProgressLeave
!insertmacro MUI_PAGE_INSTFILES

; Prepare the shared Modern UI interface variables and full-window helpers.
!insertmacro MUI_PAGE_INIT
!insertmacro MUI_PAGE_FUNCTION_FULLWINDOW

; 7. Matching completion page, shown only after installation succeeds
Page custom PageFinish PageFinishLeave

; Uninstaller Pages
; 1. Confirm uninstall page
Var DeleteAppDataCheckbox
Var DeleteAppDataCheckboxState
Var UninstallDialog
Var UninstallPathControl
Var UninstallTitleFont
UninstPage custom un.ConfirmPage un.ConfirmLeave
Function un.ConfirmPage
  Call un.SkipIfPassive
  ; Use the same full-window content area as the setup/completion pages.
  nsDialogs::Create 1044
  Pop $UninstallDialog
  ${If} $UninstallDialog == error
    Abort
  ${EndIf}
  Push $UninstallDialog
  Push 0
  Call un.PeerCastPrepareFullWindow
  SetCtlColors $UninstallDialog "241C35" "F8F6FD"
  ${If} $(^RTL) = 1
    nsDialogs::SetRTL 1
  ${EndIf}

  GetDlgItem $0 $HWNDPARENT 1
  SendMessage $0 ${WM_SETTEXT} 0 "STR:$(uninstallerConfirmButton)"
  InitPluginsDir
  File /oname=$PLUGINSDIR\peercast-uninstall-sidebar.bmp "${PEERCAST_UNINSTALLRAIL}"
  ${NSD_CreateBitmap} 5u 5u 96u 190u ""
  Pop $0
  ${NSD_SetStretchedBitmap} $0 "$PLUGINSDIR\peercast-uninstall-sidebar.bmp" $UninstallLogoHandle
  CreateFont $UninstallSmallFont "Yu Gothic UI" 8 700
  CreateFont $UninstallTitleFont "Yu Gothic UI" 13 700
  CreateFont $UninstallBodyFont "Yu Gothic UI" 9 400
  ${NSD_CreateLabel} 120u 18u $InstallerContentWidth 15u "$(uninstallerSubtitle)"
  Pop $0
  SetCtlColors $0 "6F56D9" "F8F6FD"
  SendMessage $0 ${WM_SETFONT} $UninstallSmallFont 1
  ${NSD_CreateHLine} 120u 42u $InstallerContentWidth 1u ""
  Pop $0
  ${NSD_CreateLabel} 120u 56u $InstallerContentWidth 24u "$(uninstallerTitle)"
  Pop $0
  SetCtlColors $0 "17131F" "F8F6FD"
  SendMessage $0 ${WM_SETFONT} $UninstallTitleFont 1
  ${NSD_CreateLabel} 120u 91u $InstallerContentWidth 14u "$(uninstallerLocation)"
  Pop $0
  SetCtlColors $0 "514A5E" "F8F6FD"
  SendMessage $0 ${WM_SETFONT} $UninstallBodyFont 1
  ${NSD_CreateText} 120u 109u $InstallerContentWidth 18u "$INSTDIR"
  Pop $UninstallPathControl
  SetCtlColors $UninstallPathControl "514A5E" "F8F6FD"
  SendMessage $UninstallPathControl ${WM_SETFONT} $UninstallBodyFont 1
  ${NSD_Edit_SetReadOnly} $UninstallPathControl 1
  ${NSD_CreateLabel} 123u 140u 3u 24u ""
  Pop $0
  SetCtlColors $0 "6F56D9" "6F56D9"
  ${NSD_CreateLabel} 132u 145u 270u 16u "$(uninstallerProtection)"
  Pop $0
  SetCtlColors $0 "302A3C" "F8F6FD"
  SendMessage $0 ${WM_SETFONT} $UninstallBodyFont 1

  ${NSD_CreateCheckBox} 120u 174u $InstallerContentWidth 14u "$(uninstallerDeleteCache)"
  Pop $DeleteAppDataCheckbox
  SetCtlColors $DeleteAppDataCheckbox "302A3C" "F8F6FD"
  ${NSD_AddStyle} $DeleteAppDataCheckbox ${WS_CLIPSIBLINGS}
  ${NSD_SetState} $DeleteAppDataCheckbox ${BST_UNCHECKED}
  nsDialogs::Show
  Call un.muiPageUnloadFullWindow
  ${NSD_FreeBitmap} $UninstallLogoHandle
  System::Call 'gdi32::DeleteObject(p $UninstallTitleFont)'
  System::Call 'gdi32::DeleteObject(p $UninstallSmallFont)'
  System::Call 'gdi32::DeleteObject(p $UninstallBodyFont)'
FunctionEnd
Function un.ConfirmLeave
  SendMessage $DeleteAppDataCheckbox ${BM_GETCHECK} 0 0 $DeleteAppDataCheckboxState
FunctionEnd
; 2. Uninstalling Page
!insertmacro MUI_UNPAGE_INIT
!insertmacro MUI_PAGE_FUNCTION_FULLWINDOW
!define MUI_PAGE_CUSTOMFUNCTION_SHOW un.PeerCastProgressShow
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE un.PeerCastProgressLeave
!insertmacro MUI_UNPAGE_INSTFILES

;Languages
{{#each languages}}
!insertmacro MUI_LANGUAGE "{{this}}"
{{/each}}
!insertmacro MUI_RESERVEFILE_LANGDLL
{{#each language_files}}
  !include "{{this}}"
{{/each}}

SetFont /LANG=${LANG_ENGLISH} "Segoe UI" 9
SetFont /LANG=${LANG_JAPANESE} "Yu Gothic UI" 9

LangString installerTitle ${LANG_ENGLISH} "Welcome to PeerCast RT Setup"
LangString installerTitle ${LANG_JAPANESE} "PeerCast RT セットアップへようこそ"
LangString installerSubtitle ${LANG_ENGLISH} "Stream and watch with one Windows app."
LangString installerSubtitle ${LANG_JAPANESE} "配信と視聴を、ひとつのアプリで。"
LangString installerLocation ${LANG_ENGLISH} "Install location"
LangString installerLocation ${LANG_JAPANESE} "インストール先"
LangString installerBrowse ${LANG_ENGLISH} "Browse..."
LangString installerBrowse ${LANG_JAPANESE} "参照..."
LangString installerLaunch ${LANG_ENGLISH} "Launch PeerCast RT after installation"
LangString installerLaunch ${LANG_JAPANESE} "インストール後に PeerCast RT を起動する"
LangString installerDesktop ${LANG_ENGLISH} "Create a desktop shortcut"
LangString installerDesktop ${LANG_JAPANESE} "デスクトップにショートカットを作成する"
LangString installerProtection ${LANG_ENGLISH} "Close other apps before continuing so required files can be updated safely."
LangString installerProtection ${LANG_JAPANESE} "開始する前に、ほかのアプリを終了してください。"
LangString installerProtectionNote ${LANG_ENGLISH} "Setup can safely update required files and launch PeerCast RT when it is done."
LangString installerProtectionNote ${LANG_JAPANESE} "必要なファイルを安全に更新し、完了後すぐに起動できます。"
LangString installerFirstRun ${LANG_ENGLISH} "When you are ready, select Next."
LangString installerFirstRun ${LANG_JAPANESE} "準備ができたら「次へ」をクリックしてください。"
LangString installerComplete ${LANG_ENGLISH} "PeerCast RT is ready to use."
LangString installerComplete ${LANG_JAPANESE} "PeerCast RT を使用する準備ができました。"
LangString installerProgressTitle ${LANG_ENGLISH} "Installing PeerCast RT"
LangString installerProgressTitle ${LANG_JAPANESE} "PeerCast RT をインストールしています"
LangString uninstallerProgressTitle ${LANG_ENGLISH} "Uninstalling PeerCast RT"
LangString uninstallerProgressTitle ${LANG_JAPANESE} "PeerCast RT をアンインストールしています"
LangString installerFinish ${LANG_ENGLISH} "Finish"
LangString installerFinish ${LANG_JAPANESE} "完了"
LangString installerBrowseTitle ${LANG_ENGLISH} "Select the installation folder"
LangString installerBrowseTitle ${LANG_JAPANESE} "インストール先フォルダーを選択"
LangString installerInstall ${LANG_ENGLISH} "Install"
LangString installerInstall ${LANG_JAPANESE} "インストール"
LangString installerUpdateTitle ${LANG_ENGLISH} "Update PeerCast RT"
LangString installerUpdateTitle ${LANG_JAPANESE} "PeerCast RT を更新"
LangString installerReinstallTitle ${LANG_ENGLISH} "Reinstall PeerCast RT"
LangString installerReinstallTitle ${LANG_JAPANESE} "PeerCast RT を再インストール"
LangString installerDowngradeTitle ${LANG_ENGLISH} "Install an earlier version"
LangString installerDowngradeTitle ${LANG_JAPANESE} "旧バージョンをインストール"
LangString installerExistingNote ${LANG_ENGLISH} "Your settings and recordings will be kept."
LangString installerExistingNote ${LANG_JAPANESE} "設定と録画ファイルはそのまま引き継がれます。"
LangString installerDowngradeBlocked ${LANG_ENGLISH} "A newer version is already installed. This installer cannot downgrade it."
LangString installerDowngradeBlocked ${LANG_JAPANESE} "新しいバージョンがインストール済みのため、このインストーラーでは旧バージョンに戻せません。"
LangString installerInstallError ${LANG_ENGLISH} "PeerCast RT could not be installed. Check the details above and try again."
LangString installerInstallError ${LANG_JAPANESE} "PeerCast RT をインストールできませんでした。上の詳細を確認して、もう一度お試しください。"
LangString uninstallerTitle ${LANG_ENGLISH} "Uninstall PeerCast RT"
LangString uninstallerTitle ${LANG_JAPANESE} "PeerCast RT をアンインストール"
LangString uninstallerSubtitle ${LANG_ENGLISH} "Review what will be removed before continuing."
LangString uninstallerSubtitle ${LANG_JAPANESE} "削除される内容を確認してから続行してください。"
LangString uninstallerLocation ${LANG_ENGLISH} "Installation folder"
LangString uninstallerLocation ${LANG_JAPANESE} "インストール先"
LangString uninstallerProtection ${LANG_ENGLISH} "Recordings remain in their original folders.$\r$\nSettings are kept unless you remove the app data below."
LangString uninstallerProtection ${LANG_JAPANESE} "録画ファイルは元のフォルダーに残ります。$\r$\n下の項目を選ばない限り、設定も保持されます。"
LangString uninstallerDeleteCache ${LANG_ENGLISH} "Also remove PeerCast RT settings and web cache"
LangString uninstallerDeleteCache ${LANG_JAPANESE} "PeerCast RT の設定と Web キャッシュも削除する"
LangString uninstallerConfirmButton ${LANG_ENGLISH} "Uninstall"
LangString uninstallerConfirmButton ${LANG_JAPANESE} "アンインストール"
LangString installerMachineInstallRequiresAdmin ${LANG_ENGLISH} "The existing installation is for all users. Run this installer as an administrator to update it."
LangString installerMachineInstallRequiresAdmin ${LANG_JAPANESE} "このアプリは全ユーザー向けにインストールされています。更新するには管理者としてインストーラーを実行してください。"
LangString invalidInstallPath ${LANG_ENGLISH} "Choose a valid installation folder below a drive or network share root."
LangString invalidInstallPath ${LANG_JAPANESE} "ドライブまたはネットワーク共有の直下ではなく、有効なインストール先フォルダーを選択してください。"

Function PageReady
  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    Abort
  ${EndIf}

  nsDialogs::Create 1044
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  StrCpy $ReadyDialog $0
  StrCpy $ReadyBackNavigation 0

  ; MUI's outer window and the nsDialogs page are separate Win32 windows.
  ; Expand the child page before creating right-anchored controls; resizing
  ; only the outer frame leaves an empty strip on the right at high DPI.
  Call muiPageLoadFullWindow
  ShowWindow $mui.Line.FullWindow ${SW_HIDE}
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetClientRect(p $HWNDPARENT, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  StrCpy $R0 $3
  System::Free $0
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetWindowRect(p $ReadyDialog, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  IntOp $R1 $4 - $2
  System::Free $0
  System::Call 'user32::SetWindowPos(p $ReadyDialog, p 0, i 0, i 0, i $R0, i $R1, i 0x0006)'
  Push $ReadyDialog
  Call PeerCastMeasureContent

  GetDlgItem $0 $HWNDPARENT 3
  ShowWindow $0 ${SW_HIDE}
  !if "${STARTMENUFOLDER}" == ""
    GetDlgItem $0 $HWNDPARENT 1
    SendMessage $0 ${WM_SETTEXT} 0 "STR:$(installerInstall)"
  !endif
  SetCtlColors $ReadyDialog "241C35" "F8F6FD"
  ${If} $(^RTL) = 1
    nsDialogs::SetRTL 1
  ${EndIf}

  ; The brand rail is a single pre-rendered bitmap so the original loop logo
  ; keeps its proportions and antialiasing at every installer build.
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File /oname=peercast-installer-sidebar.bmp "${PEERCAST_BRANDRAIL}"
  ${NSD_CreateBitmap} 5u 5u 96u 190u ""
  Pop $2
  ${NSD_SetStretchedBitmap} $2 "$PLUGINSDIR\peercast-installer-sidebar.bmp" $InstallerLogoHandle

  ; A continuous pale surface keeps native label rectangles and the brand
  ; artwork from reading as separate white panels.

  CreateFont $InstallerSmallFont "Yu Gothic UI" 8 700
  CreateFont $InstallerTitleFont "Yu Gothic UI" 13 700
  CreateFont $InstallerHeadingFont "Yu Gothic UI" 9 700
  CreateFont $InstallerBodyFont "Yu Gothic UI" 9 400

  ${NSD_CreateLabel} 120u 15u 84u 12u "セットアップ"
  Pop $3
  SetCtlColors $3 "6F56D9" "F8F6FD"
  SendMessage $3 ${WM_SETFONT} $InstallerSmallFont 1

  ${If} $ReadyExistingMode = 1
    StrCpy $0 "$(installerUpdateTitle)"
  ${ElseIf} $ReadyExistingMode = 2
    StrCpy $0 "$(installerReinstallTitle)"
  ${ElseIf} $ReadyExistingMode = 3
    StrCpy $0 "$(installerDowngradeTitle)"
  ${Else}
    StrCpy $0 "$(installerTitle)"
  ${EndIf}
  ${NSD_CreateLabel} 120u 47u $InstallerContentWidth 27u "$0"
  Pop $3
  SetCtlColors $3 "17131F" "F8F6FD"
  SendMessage $3 ${WM_SETFONT} $InstallerTitleFont 1

  ${NSD_CreateLabel} 120u 78u $InstallerContentWidth 15u "PeerCast RTをこのPCにインストールします。"
  Pop $3
  SetCtlColors $3 "514A5E" "F8F6FD"
  SendMessage $3 ${WM_SETFONT} $InstallerBodyFont 1
  ${NSD_CreateLabel} 120u 94u $InstallerContentWidth 15u "必要なファイルをこのPCに準備します。"
  Pop $3
  SetCtlColors $3 "514A5E" "F8F6FD"
  SendMessage $3 ${WM_SETFONT} $InstallerBodyFont 1

  ${NSD_CreateLabel} 129u 125u 280u 15u "$(installerProtection)"
  Pop $3
  SetCtlColors $3 "302A3C" "F8F6FD"
  SendMessage $3 ${WM_SETFONT} $InstallerHeadingFont 1
  ${NSD_CreateLabel} 129u 143u 280u 13u "$(installerProtectionNote)"
  Pop $3
  SetCtlColors $3 "7C748A" "F8F6FD"
  SendMessage $3 ${WM_SETFONT} $InstallerSmallFont 1

  ${If} $ReadyExistingMode != 0
    ${NSD_CreateLabel} 120u 167u $InstallerContentWidth 12u "$(installerExistingNote)"
    Pop $3
    SetCtlColors $3 "514A5E" "F8F6FD"
    SendMessage $3 ${WM_SETFONT} $InstallerSmallFont 1
  ${Else}
    ${NSD_CreateLabel} 120u 171u $InstallerContentWidth 13u "$(installerFirstRun)"
    Pop $3
    SetCtlColors $3 "514A5E" "F8F6FD"
    SendMessage $3 ${WM_SETFONT} $InstallerBodyFont 1
  ${EndIf}

  nsDialogs::Show
  Call muiPageUnloadFullWindow
  ${NSD_FreeBitmap} $InstallerLogoHandle
  System::Call 'gdi32::DeleteObject(p $InstallerTitleFont)'
  System::Call 'gdi32::DeleteObject(p $InstallerHeadingFont)'
  System::Call 'gdi32::DeleteObject(p $InstallerBodyFont)'
  System::Call 'gdi32::DeleteObject(p $InstallerSmallFont)'
FunctionEnd

Function PageReadyBrowse
  Pop $0
  nsDialogs::SelectFolderDialog "$(installerBrowseTitle)" "$INSTDIR"
  Pop $0
  ${If} $0 != "error"
    StrCpy $INSTDIR $0
    ${NSD_SetText} $ReadyPathControl "$INSTDIR"
  ${EndIf}
FunctionEnd

Function PageReadyLeave
  Call ValidateInstallPath
  ${If} $0 != 1
    MessageBox MB_OK|MB_ICONEXCLAMATION "$(invalidInstallPath)"
    Abort
  ${EndIf}
  ${If} $NoShortcutMode = 1
    StrCpy $ReadyDesktop 0
  ${EndIf}
FunctionEnd

Function PageReadyBack
  StrCpy $ReadyBackNavigation 1
FunctionEnd

Function ValidateInstallPath
  StrCmp $INSTDIR "" invalid
  StrCpy $1 $INSTDIR 2
  StrCmp $1 "\\" validate_unc
  StrCpy $1 $INSTDIR 1 1
  StrCmp $1 ":" 0 invalid
  StrCpy $1 $INSTDIR 1 2
  StrCmp $1 "\" 0 invalid
  Goto validate_components

  validate_unc:
  ; A UNC path must have both a server and a share component.
  StrCpy $4 $INSTDIR "" 2
  ${StrLoc} $5 $4 "\" ">"
  StrCmp $5 "" invalid
  StrCmp $5 0 invalid
  IntOp $5 $5 + 1
  StrCpy $6 $4 1 $5
  StrCmp $6 "" invalid
  StrCmp $6 "\" invalid

  validate_components:
  ; Reject dot path components before canonicalization can hide them.
  StrCpy $4 "$INSTDIR\"
  ${StrLoc} $5 $4 "\.\" ">"
  StrCmp $5 "" +2
  Goto invalid
  ${StrLoc} $5 $4 "\..\" ">"
  StrCmp $5 "" +2
  Goto invalid

  ; GetFullPathName (the NSIS instruction) depends on the target existing.
  ; The Win32 API performs lexical canonicalization for a not-yet-created path.
  ; NSIS strings are limited to 1024 characters; keep the API buffer and
  ; System plug-in copy within that limit instead of reading a Win32 MAX_PATH
  ; sized result into a much smaller NSIS string.
  System::Alloc 2048
  Pop $4
  StrCmp $4 "error" invalid
  StrCmp $4 "0" invalid
  System::Call 'kernel32::GetFullPathNameW(w "$INSTDIR", i 1024, p $4, p 0)i.r5'
  ${If} $5 = 0
    System::Free $4
    Goto invalid
  ${EndIf}
  ${If} $5 >= 1024
    System::Free $4
    Goto invalid
  ${EndIf}
  System::Call '*$4(&w1024 .r1)'
  System::Free $4
  StrCmp $1 "" invalid
  ${GetRoot} "$1" $2
  StrCmp $2 "" invalid
  StrCmp $1 $2 invalid
  StrCpy $3 "$2\"
  StrCmp $1 $3 invalid
  StrCpy $0 1
  Return

  invalid:
  StrCpy $0 0
FunctionEnd

Function .onVerifyInstDir
  Call ValidateInstallPath
  ${If} $0 != 1
    Abort
  ${EndIf}
FunctionEnd

Function PageFinish
  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
  ${OrIf} $InstallSucceeded != 1
    Abort
  ${EndIf}
  nsDialogs::Create 1044
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  StrCpy $FinishDialog $0
  StrCpy $FinishBackNavigation 0
  Call muiPageLoadFullWindow
  ShowWindow $mui.Line.FullWindow ${SW_HIDE}
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetClientRect(p $HWNDPARENT, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  StrCpy $R0 $3
  System::Free $0
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetWindowRect(p $FinishDialog, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  IntOp $R1 $4 - $2
  System::Free $0
  System::Call 'user32::SetWindowPos(p $FinishDialog, p 0, i 0, i 0, i $R0, i $R1, i 0x0006)'
  Push $FinishDialog
  Call PeerCastMeasureContent
  GetDlgItem $0 $HWNDPARENT 3
  ShowWindow $0 ${SW_HIDE}
  GetDlgItem $0 $HWNDPARENT 1
  SendMessage $0 ${WM_SETTEXT} 0 "STR:$(installerFinish)"
  SetCtlColors $FinishDialog "241C35" "F8F6FD"
  ${If} $(^RTL) = 1
    nsDialogs::SetRTL 1
  ${EndIf}

  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File /oname=peercast-installer-finish-sidebar.bmp "${PEERCAST_BRANDRAIL}"
  ${NSD_CreateBitmap} 5u 5u 96u 190u ""
  Pop $2
  ${NSD_SetStretchedBitmap} $2 "$PLUGINSDIR\peercast-installer-finish-sidebar.bmp" $InstallerLogoHandle
  CreateFont $InstallerTitleFont "Yu Gothic UI" 13 700
  CreateFont $InstallerBodyFont "Yu Gothic UI" 9 400
  CreateFont $InstallerSmallFont "Yu Gothic UI" 8 700
  ${NSD_CreateLabel} 120u 16u 90u 12u "セットアップ完了"
  Pop $3
  SetCtlColors $3 "6F56D9" "F8F6FD"
  SendMessage $3 ${WM_SETFONT} $InstallerSmallFont 1
  ${NSD_CreateLabel} 120u 49u $InstallerContentWidth 28u "$(installerComplete)"
  Pop $3
  SetCtlColors $3 "17131F" "F8F6FD"
  SendMessage $3 ${WM_SETFONT} $InstallerTitleFont 1
  ${NSD_CreateLabel} 129u 98u 280u 14u "設定と録画ファイルはそのまま保持されています。"
  Pop $3
  SetCtlColors $3 "514A5E" "F8F6FD"
  SendMessage $3 ${WM_SETFONT} $InstallerBodyFont 1
  ${NSD_CreateCheckBox} 120u 139u $InstallerContentWidth 14u "$(installerLaunch)"
  Pop $FinishLaunchCheckbox
  SetCtlColors $FinishLaunchCheckbox "302A3C" "F8F6FD"
  ${NSD_AddStyle} $FinishLaunchCheckbox ${WS_CLIPSIBLINGS}
  ${NSD_CreateCheckBox} 120u 156u $InstallerContentWidth 14u "$(installerDesktop)"
  Pop $FinishDesktopCheckbox
  SetCtlColors $FinishDesktopCheckbox "302A3C" "F8F6FD"
  ${NSD_AddStyle} $FinishDesktopCheckbox ${WS_CLIPSIBLINGS}
  ${If} $NoShortcutMode = 1
    EnableWindow $FinishDesktopCheckbox 0
  ${EndIf}
  ${NSD_CreateLabel} 120u 177u $InstallerContentWidth 13u "「完了」をクリックしてセットアップを閉じます。"
  Pop $3
  SetCtlColors $3 "514A5E" "F8F6FD"
  SendMessage $3 ${WM_SETFONT} $InstallerBodyFont 1
  ${If} $ReadyLaunch = 1
    ${NSD_SetState} $FinishLaunchCheckbox ${BST_CHECKED}
  ${Else}
    ${NSD_SetState} $FinishLaunchCheckbox ${BST_UNCHECKED}
  ${EndIf}
  ${If} $ReadyDesktop = 1
    ${NSD_SetState} $FinishDesktopCheckbox ${BST_CHECKED}
  ${Else}
    ${NSD_SetState} $FinishDesktopCheckbox ${BST_UNCHECKED}
  ${EndIf}
  nsDialogs::Show
  Call muiPageUnloadFullWindow
  ${NSD_FreeBitmap} $InstallerLogoHandle
  System::Call 'gdi32::DeleteObject(p $InstallerTitleFont)'
  System::Call 'gdi32::DeleteObject(p $InstallerBodyFont)'
  System::Call 'gdi32::DeleteObject(p $InstallerSmallFont)'
FunctionEnd

Function PageFinishLeave
  ${If} $FinishBackNavigation = 1
    StrCpy $FinishBackNavigation 0
    ${NSD_GetState} $FinishLaunchCheckbox $ReadyLaunch
    ${NSD_GetState} $FinishDesktopCheckbox $ReadyDesktop
    Return
  ${EndIf}
  ${If} $InstallSucceeded != 1
    Return
  ${EndIf}
  ${If} $FinishActionsRun = 1
    Return
  ${EndIf}
  StrCpy $FinishActionsRun 1
  ${NSD_GetState} $FinishDesktopCheckbox $0
  ${If} $0 = 1
  ${AndIf} $NoShortcutMode != 1
    Call CreateOrUpdateDesktopShortcut
  ${EndIf}
  ${NSD_GetState} $FinishLaunchCheckbox $0
  ${If} $0 = 1
    Call RunMainBinary
  ${EndIf}
FunctionEnd

Function PageFinishBack
  ${NSD_GetState} $FinishLaunchCheckbox $ReadyLaunch
  ${NSD_GetState} $FinishDesktopCheckbox $ReadyDesktop
  StrCpy $FinishBackNavigation 1
FunctionEnd

Function RunMainBinary
  nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
FunctionEnd

; Measure pixel widths after the page has its final client dimensions.
; Negative nsDialogs widths are relative to the entire dialog, not to X.
!macro PeerCastMeasureContentBody
  Pop $9
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetClientRect(p $9, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  StrCpy $8 $3
  System::Free $0
  System::Call '*(i 0, i 0, i 140, i 0)p.r0'
  System::Call 'user32::MapDialogRect(p $HWNDPARENT, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  IntOp $InstallerContentWidth $8 - $3
  System::Free $0
  System::Call '*(i 0, i 0, i 112, i 0)p.r0'
  System::Call 'user32::MapDialogRect(p $HWNDPARENT, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  IntOp $InstallerCardWidth $8 - $3
  System::Free $0
!macroend
Function PeerCastMeasureContent
  !insertmacro PeerCastMeasureContentBody
FunctionEnd
Function un.PeerCastMeasureContent
  !insertmacro PeerCastMeasureContentBody
FunctionEnd

; Shared full-window sizing for installer and uninstaller pages. Stack input:
; page HWND, then desired height in dialog units (0 preserves its current height).
!macro PeerCastPrepareFullWindow PREFIX
Function ${PREFIX}PeerCastPrepareFullWindow
  Pop $8
  Pop $9
  Call ${PREFIX}muiPageLoadFullWindow
  ShowWindow $mui.Line.FullWindow ${SW_HIDE}
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetClientRect(p $HWNDPARENT, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  StrCpy $7 $3
  System::Free $0
  ${If} $8 = 0
    System::Call '*(i 0, i 0, i 0, i 0)p.r0'
    System::Call 'user32::GetWindowRect(p $9, p r0)'
    System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
    IntOp $6 $4 - $2
  ${Else}
    System::Call '*(i 0, i 0, i 0, i $8)p.r0'
    System::Call 'user32::MapDialogRect(p $HWNDPARENT, p r0)'
    System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
    StrCpy $6 $4
  ${EndIf}
  System::Call 'user32::SetWindowPos(p $9, p 0, i 0, i 0, i $7, i $6, i 0x0006)'
  System::Free $0
  Push $9
  Call ${PREFIX}PeerCastMeasureContent
FunctionEnd
!macroend
!insertmacro PeerCastPrepareFullWindow ""
!insertmacro PeerCastPrepareFullWindow "un."

!macro PeerCastMove CONTROL X Y W H
  System::Call '*(i ${X}, i ${Y}, i ${W}, i ${H})p.r0'
  System::Call 'user32::MapDialogRect(p $HWNDPARENT, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  System::Call 'user32::SetWindowPos(p ${CONTROL}, p 0, i r1, i r2, i r3, i r4, i 0x0004)'
  System::Free $0
!macroend

; Keep NSIS's actual install/uninstall controls and section execution, while
; using the same full-window visual system on both progress pages.
!macro PeerCastProgressShowBody PREFIX TITLE
  Call ${PREFIX}muiPageLoadFullWindow
  ShowWindow $mui.Line.FullWindow ${SW_HIDE}
  SetCtlColors $mui.InstFilesPage "241C35" "F8F6FD"
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetClientRect(p $HWNDPARENT, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  StrCpy $8 $3
  System::Free $0
  System::Call '*(i 0, i 0, i 0, i 200)p.r0'
  System::Call 'user32::MapDialogRect(p $HWNDPARENT, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  System::Call 'user32::SetWindowPos(p $mui.InstFilesPage, p 0, i 0, i 0, i r8, i r4, i 0x0004)'
  System::Free $0
  Push $mui.InstFilesPage
  Call ${PREFIX}PeerCastMeasureContent
  CreateFont $ProgressTitleFont "Yu Gothic UI" 13 700
  CreateFont $ProgressBodyFont "Yu Gothic UI" 9 400
  InitPluginsDir
  !if "${PREFIX}" == "un."
    File /oname=$PLUGINSDIR\peercast-progress.bmp "${PEERCAST_UNINSTALLRAIL}"
  !else
    File /oname=$PLUGINSDIR\peercast-progress.bmp "${PEERCAST_PROGRESSRAIL}"
  !endif
  System::Call 'user32::CreateWindowExW(i 0, w "STATIC", w "", i 0x5400000E, i 0, i 0, i 0, i 0, p $mui.InstFilesPage, p 1200, p 0, p 0)p.r9'
  !insertmacro PeerCastMove $9 5 5 96 190
  ${NSD_SetStretchedBitmap} $9 "$PLUGINSDIR\peercast-progress.bmp" $ProgressLogoHandle
  System::Call 'user32::CreateWindowExW(i 0, w "STATIC", w "${TITLE}", i 0x54000000, i 0, i 0, i 0, i 0, p $mui.InstFilesPage, p 1202, p 0, p 0)p.r9'
  SetCtlColors $9 "17131F" "F8F6FD"
  SendMessage $9 ${WM_SETFONT} $ProgressTitleFont 1
  !insertmacro PeerCastMove $9 120 47 280 25
  System::Call 'user32::SetWindowPos(p r9, p 0, i 0, i 0, i 0, i 0, i 0x0003)'
  !insertmacro PeerCastMove $mui.InstFilesPage.Text 120 82 280 14
  SetCtlColors $mui.InstFilesPage.Text "514A5E" "F8F6FD"
  SendMessage $mui.InstFilesPage.Text ${WM_SETFONT} $ProgressBodyFont 1
  !insertmacro PeerCastMove $mui.InstFilesPage.ProgressBar 120 104 280 9
  System::Call 'uxtheme::SetWindowTheme(p $mui.InstFilesPage.ProgressBar, w "", w "")'
  SendMessage $mui.InstFilesPage.ProgressBar 0x0409 0 0x00D9566F
  SendMessage $mui.InstFilesPage.ProgressBar 0x2001 0 0x00EDE4E8
  !insertmacro PeerCastMove $mui.InstFilesPage.ShowLogButton 120 125 80 16
  !insertmacro PeerCastMove $mui.InstFilesPage.Log 120 147 280 38
  SetCtlColors $mui.InstFilesPage.Log "514A5E" "F8F6FD"
  ; Static backgrounds must stay behind the built-in progress controls.
  System::Call 'user32::SetWindowPos(p $mui.InstFilesPage.Text, p 0, i 0, i 0, i 0, i 0, i 0x0003)'
  System::Call 'user32::SetWindowPos(p $mui.InstFilesPage.ProgressBar, p 0, i 0, i 0, i 0, i 0, i 0x0003)'
  System::Call 'user32::SetWindowPos(p $mui.InstFilesPage.ShowLogButton, p 0, i 0, i 0, i 0, i 0, i 0x0003)'
  System::Call 'user32::SetWindowPos(p $mui.InstFilesPage.Log, p 0, i 0, i 0, i 0, i 0, i 0x0003)'
  GetDlgItem $0 $HWNDPARENT 3
  ShowWindow $0 ${SW_HIDE}
!macroend

Function PeerCastProgressShow
  !insertmacro PeerCastProgressShowBody "" "$(installerProgressTitle)"
FunctionEnd
Function un.PeerCastProgressShow
  !insertmacro PeerCastProgressShowBody "un." "$(uninstallerProgressTitle)"
FunctionEnd
Function PeerCastProgressLeave
  ; NSIS calls the instfiles leave callback when sections finish, while
  ; this page remains visible awaiting Next. Keep its bitmap, fonts and
  ; full-window chrome alive until the actual completion page is closed.
FunctionEnd
Function un.PeerCastProgressLeave
  ; Keep the progress resources alive while the uninstaller completion page
  ; is still visible, matching the installer callback behavior.
FunctionEnd

!macro PeerCastGUIInitBody
  SetCtlColors $HWNDPARENT "241C35" "F8F6FD"
  SetCtlColors $mui.Branding.Text "7C748A" "F8F6FD"
  SetCtlColors $mui.Branding.Background "241C35" "F8F6FD"
  ; Use the same pale surface for the frame, footer, custom pages and labels.
  System::Call 'dwmapi::DwmSetWindowAttribute(p $HWNDPARENT, i 35, *i 0x00FDF6F8, i 4)'
  System::Call 'dwmapi::DwmSetWindowAttribute(p $HWNDPARENT, i 36, *i 0x00241C35, i 4)'

  ; The stock NSIS dialog becomes too tall and narrow on high-DPI displays.
  ; Preserve its measured height, widen it to the approved mockup's 1.55:1
  ; aspect ratio, and keep the window centered horizontally.
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetWindowRect(p $HWNDPARENT, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  IntOp $5 $3 - $1
  IntOp $6 $4 - $2
  IntOp $7 $6 * 155
  IntOp $7 $7 / 100
  IntOp $8 $7 - $5
  IntOp $8 $8 / 2
  IntOp $1 $1 - $8
  System::Call 'user32::SetWindowPos(p $HWNDPARENT, p 0, i $1, i $2, i $7, i $6, i 0x0004)'
  System::Free $0

  ; Re-anchor the native footer actions to the widened window.
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetClientRect(p $HWNDPARENT, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  StrCpy $R0 $3
  StrCpy $R1 $4
  System::Free $0

  GetDlgItem $9 $HWNDPARENT 2
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetWindowRect(p $9, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  IntOp $5 $3 - $1
  IntOp $6 $4 - $2
  IntOp $7 $R0 - $5
  IntOp $7 $7 - 18
  IntOp $8 $R1 - $6
  IntOp $8 $8 - 14
  System::Call 'user32::SetWindowPos(p $9, p 0, i $7, i $8, i $5, i $6, i 0x0004)'
  System::Free $0

  GetDlgItem $9 $HWNDPARENT 1
  System::Call '*(i 0, i 0, i 0, i 0)p.r0'
  System::Call 'user32::GetWindowRect(p $9, p r0)'
  System::Call '*$0(i.r1, i.r2, i.r3, i.r4)'
  IntOp $5 $3 - $1
  IntOp $6 $4 - $2
  IntOp $7 $R0 - $5
  IntOp $7 $7 - 150
  IntOp $8 $R1 - $6
  IntOp $8 $8 - 14
  System::Call 'user32::SetWindowPos(p $9, p 0, i $7, i $8, i $5, i $6, i 0x0004)'
  System::Free $0
!macroend
Function PeerCastGUIInit
  !insertmacro PeerCastGUIInitBody
FunctionEnd
Function un.PeerCastGUIInit
  !insertmacro PeerCastGUIInitBody
FunctionEnd


Function .onInit
  StrCpy $ReadyLaunch 1
  StrCpy $ReadyDesktop 0
  StrCpy $AppStartMenuFolder "${STARTMENUFOLDER}"
  StrCpy $ReadyExistingMode 0
  StrCpy $InstallSucceeded 0
  StrCpy $FinishActionsRun 0
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

  ; Tauri's updater launches the installer with this marker. Treat it as a
  ; passive in-place update so existing install context and app data survive.
  ${GetOptions} $CMDLINE "/UPDATER" $0
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
    StrCpy $PassiveMode 1
  ${EndIf}

  ${If} $PassiveMode != 1
  ${AndIf} $UpdateMode != 1
  !if "${DISPLAYLANGUAGESELECTOR}" == "true"
    !insertmacro MUI_LANGDLL_DISPLAY
  !endif
  ${EndIf}

  !insertmacro SetContext

  ${If} $INSTDIR == "${PLACEHOLDER_INSTALL_DIR}"
    ; Set default install location
    !if "${INSTALLMODE}" == "perMachine"
      ${If} ${RunningX64}
        !if "${ARCH}" == "x64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else if "${ARCH}" == "arm64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else
          StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
        !endif
      ${Else}
        StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
      ${EndIf}
    !else if "${INSTALLMODE}" == "currentUser"
      StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}"
    !endif

    Call RestorePreviousInstallLocation
  ${EndIf}


  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_INIT

    ; Older releases were per-user and did not write MultiUser's CurrentUser
    ; marker. Prefer that existing HKCU install and restore its saved folder.
    ReadRegStr $0 HKLM "${MANUPRODUCTKEY}" ""
    ReadRegStr $1 HKCU "${MANUPRODUCTKEY}" ""
    ${If} $0 != ""
    ${AndIf} $MultiUser.InstallMode != "AllUsers"
      ${If} $MultiUser.Privileges != "Admin"
        MessageBox MB_OK|MB_ICONSTOP "$(installerMachineInstallRequiresAdmin)"
        Quit
      ${EndIf}
      Call MultiUser.InstallMode.AllUsers
    ${EndIf}
    ${If} $0 == ""
    ${AndIf} $1 != ""
      Call MultiUser.InstallMode.CurrentUser
    ${EndIf}
  !endif
FunctionEnd


Section EarlyChecks
  ; Abort silent installer if downgrades is disabled
  !if "${ALLOWDOWNGRADES}" == "false"
  ${If} ${Silent}
    ; If downgrading
    ${If} $R0 = -1
      System::Call 'kernel32::AttachConsole(i -1)i.r0'
      ${If} $0 <> 0
        System::Call 'kernel32::GetStdHandle(i -11)i.r0'
        System::call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)' ; set red color
        FileWrite $0 "$(silentDowngrades)"
      ${EndIf}
      Abort
    ${EndIf}
  ${EndIf}
  !endif

SectionEnd

Section WebView2
  ; Check if Webview2 is already installed and skip this section
  ${If} ${RunningX64}
    ReadRegStr $4 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${Else}
    ReadRegStr $4 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}
  ${If} $4 == ""
    ReadRegStr $4 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}

  ${If} $4 == ""
    ; Webview2 installation
    ;
    ; Skip if updating
    ${If} $UpdateMode <> 1
      !if "${INSTALLWEBVIEW2MODE}" == "downloadBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        DetailPrint "$(webview2Downloading)"
        NSISdl::download "https://go.microsoft.com/fwlink/p/?LinkId=2124703" "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Pop $0
        ${If} $0 == "success"
          DetailPrint "$(webview2DownloadSuccess)"
        ${Else}
          DetailPrint "$(webview2DownloadError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif

      !if "${INSTALLWEBVIEW2MODE}" == "embedBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2BOOTSTRAPPERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif

      !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
        Delete "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${WEBVIEW2INSTALLERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        Goto install_webview2
      !endif

      Goto webview2_done

      install_webview2:
        DetailPrint "$(installingWebview2)"
        ; $6 holds the path to the webview2 installer
        ExecWait "$6 ${WEBVIEW2INSTALLERARGS} /install" $1
        ${If} $1 = 0
          DetailPrint "$(webview2InstallSuccess)"
        ${Else}
          DetailPrint "$(webview2InstallError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
      webview2_done:
    ${EndIf}
  ${Else}
    !if "${MINIMUMWEBVIEW2VERSION}" != ""
      ${VersionCompare} "${MINIMUMWEBVIEW2VERSION}" "$4" $R0
      ${If} $R0 = 1
        update_webview:
          DetailPrint "$(installingWebview2)"
          ${If} ${RunningX64}
            ReadRegStr $R1 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate" "path"
          ${Else}
            ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 == ""
            ReadRegStr $R1 HKCU "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 != ""
            ; Chromium updater docs: https://source.chromium.org/chromium/chromium/src/+/main:docs/updater/user_manual.md
            ; Modified from "HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Microsoft EdgeWebView\ModifyPath"
            ExecWait `"$R1" /install appguid=${WEBVIEW2APPGUID}&needsadmin=true` $1
            ${If} $1 = 0
              DetailPrint "$(webview2InstallSuccess)"
            ${Else}
              MessageBox MB_ICONEXCLAMATION|MB_ABORTRETRYIGNORE "$(webview2InstallError)" IDIGNORE ignore IDRETRY update_webview
              Quit
              ignore:
            ${EndIf}
          ${EndIf}
      ${EndIf}
    !endif
  ${EndIf}
SectionEnd

Section Install
  ClearErrors
  SetOutPath $INSTDIR
  IfErrors install_section_failed

  !ifmacrodef NSIS_HOOK_PREINSTALL
    !insertmacro NSIS_HOOK_PREINSTALL
  !endif

  ClearErrors
  ReadRegStr $OldMainBinaryName SHCTX "${UNINSTKEY}" "MainBinaryName"
  IfErrors install_old_binary_missing
  Goto install_old_binary_checked
  install_old_binary_missing:
    StrCpy $OldMainBinaryName ""
    ClearErrors
  install_old_binary_checked:
  ${If} $OldMainBinaryName != ""
    ; This value is stored by the previous installer and must be a basename.
    ${GetFileName} "$OldMainBinaryName" $0
    ${If} $0 != $OldMainBinaryName
      Goto install_section_failed
    ${EndIf}
    ${GetFileExt} "$OldMainBinaryName" $0
    ${If} $0 != "exe"
      Goto install_section_failed
    ${EndIf}
    ${StrLoc} $0 "$OldMainBinaryName" "\" ">"
    ${If} $0 != ""
      Goto install_section_failed
    ${EndIf}
    ${StrLoc} $0 "$OldMainBinaryName" "/" ">"
    ${If} $0 != ""
      Goto install_section_failed
    ${EndIf}
    ${StrLoc} $0 "$OldMainBinaryName" ":" ">"
    ${If} $0 != ""
      Goto install_section_failed
    ${EndIf}
    ${StrLoc} $0 "$OldMainBinaryName" "*" ">"
    ${If} $0 != ""
      Goto install_section_failed
    ${EndIf}
    ${StrLoc} $0 "$OldMainBinaryName" "?" ">"
    ${If} $0 != ""
      Goto install_section_failed
    ${EndIf}
    ${If} $OldMainBinaryName != "${MAINBINARYNAME}.exe"
      !insertmacro CheckIfAppIsRunning "$INSTDIR\$OldMainBinaryName" "${PRODUCTNAME}"
    ${EndIf}
  ${EndIf}

  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  ClearErrors
  ; Copy main executable
  File "${MAINBINARYSRCPATH}"

  ; Copy resources
  {{#each resources_dirs}}
    CreateDirectory "$INSTDIR\\{{this}}"
  {{/each}}
  {{#each resources}}
    File /a "/oname={{this.[1]}}" "{{no-escape @key}}"
  {{/each}}

  ; Copy external binaries
  {{#each binaries}}
    File /a "/oname={{this}}" "{{no-escape @key}}"
  {{/each}}
  IfErrors install_section_failed
  ClearErrors

  ; Create file associations
  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
       !insertmacro APP_ASSOCIATE "{{ext}}" "{{or association.name ext}}" "{{association-description association.description ext}}" "$INSTDIR\${MAINBINARYNAME}.exe,0" "Open with ${PRODUCTNAME}" "$INSTDIR\${MAINBINARYNAME}.exe $\"%1$\""
    {{/each}}
  {{/each}}

  ; Register deep links
  {{#each deep_link_protocols as |protocol| ~}}
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "URL Protocol" ""
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "" "URL:${BUNDLEID} protocol"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  {{/each}}

  ; Create uninstaller
  ClearErrors
  WriteUninstaller "$INSTDIR\uninstall.exe"
  IfErrors install_section_failed

  ; Save $INSTDIR in registry for future installations
  ClearErrors
  WriteRegStr SHCTX "${MANUPRODUCTKEY}" "" $INSTDIR
  IfErrors install_section_failed

  !if "${INSTALLMODE}" == "both"
    ; Save install mode to be selected by default for the next installation such as updating
    ; or when uninstalling
    ClearErrors
    WriteRegStr SHCTX "${UNINSTKEY}" $MultiUser.InstallMode 1
    IfErrors install_section_failed
  !endif

  ; Remove old main binary if it doesn't match new main binary name
  ${If} $OldMainBinaryName != ""
  ${AndIf} $OldMainBinaryName != "${MAINBINARYNAME}.exe"
    ClearErrors
    Delete "$INSTDIR\$OldMainBinaryName"
    IfErrors install_section_failed
  ${EndIf}

  ; Save current MAINBINARYNAME for future updates
  ClearErrors
  WriteRegStr SHCTX "${UNINSTKEY}" "MainBinaryName" "${MAINBINARYNAME}.exe"
  IfErrors install_section_failed

  ; Registry information for add/remove programs
  ClearErrors
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  IfErrors install_section_failed
  ClearErrors
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\""
  IfErrors install_section_failed
  ClearErrors
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  IfErrors install_section_failed
  ClearErrors
  WriteRegStr SHCTX "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  IfErrors install_section_failed
  ClearErrors
  WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  IfErrors install_section_failed
  ClearErrors
  WriteRegStr SHCTX "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  IfErrors install_section_failed
  ClearErrors
  WriteRegStr SHCTX "${UNINSTKEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  IfErrors install_section_failed
  ; Explicitly expose the application to standard uninstall-list readers.
  ClearErrors
  WriteRegDWORD SHCTX "${UNINSTKEY}" "SystemComponent" "0"
  IfErrors install_section_failed
  ClearErrors
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRemove" "0"
  IfErrors install_section_failed
  ClearErrors
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoModify" "1"
  IfErrors install_section_failed
  ClearErrors
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRepair" "1"
  IfErrors install_section_failed
  StrCpy $InstallSucceeded 1

  ClearErrors
  ${GetSize} "$INSTDIR" "/M=uninstall.exe /S=0K /G=0" $0 $1 $2
  IntOp $0 $0 + ${ESTIMATEDSIZE}
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD SHCTX "${UNINSTKEY}" "EstimatedSize" "$0"

  !if "${HOMEPAGE}" != ""
    WriteRegStr SHCTX "${UNINSTKEY}" "URLInfoAbout" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "URLUpdateInfo" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "HelpLink" "${HOMEPAGE}"
  !endif

  ; Create start menu shortcut
  !insertmacro MUI_STARTMENU_WRITE_BEGIN Application
    Call CreateOrUpdateStartMenuShortcut
  !insertmacro MUI_STARTMENU_WRITE_END
  Call MigrateExistingDesktopShortcut

  !ifmacrodef NSIS_HOOK_POSTINSTALL
    !insertmacro NSIS_HOOK_POSTINSTALL
  !endif

  ; Auto close this page for passive mode
  ${If} $PassiveMode = 1
    SetAutoClose true
  ${EndIf}

  Goto install_section_done
  install_section_failed:
    StrCpy $InstallSucceeded 0
    Abort "$(installerInstallError)"
  install_section_done:
SectionEnd

Function .onInstSuccess
  ${If} $InstallSucceeded != 1
    Return
  ${EndIf}

  ; Passive and silent runs have no interactive completion page.
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${If} $NoShortcutMode != 1
      Call CreateOrUpdateDesktopShortcut
    ${EndIf}
  ${EndIf}

  ; Check for `/R` flag only in silent and passive installers because
  ; GUI installer has a toggle for the user to (re)start the app
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${GetOptions} $CMDLINE "/R" $R0
    ${IfNot} ${Errors}
      ${GetOptions} $CMDLINE "/ARGS" $R0
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" "$R0"
    ${EndIf}
  ${EndIf}
FunctionEnd

Function un.onInit
  !insertmacro SetContext

  !if "${INSTALLMODE}" == "both"
    !insertmacro MULTIUSER_UNINIT
  !endif

  !insertmacro MUI_UNGETLANGUAGE

  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATER" $0
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
    StrCpy $PassiveMode 1
  ${EndIf}
FunctionEnd

Section Uninstall

  !ifmacrodef NSIS_HOOK_PREUNINSTALL
    !insertmacro NSIS_HOOK_PREUNINSTALL
  !endif

  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  ; Delete the app directory and its content from disk
  ; Copy main executable
  Delete "$INSTDIR\${MAINBINARYNAME}.exe"

  ; Delete resources
  {{#each resources}}
    Delete "$INSTDIR\\{{this.[1]}}"
  {{/each}}

  ; Delete external binaries
  {{#each binaries}}
    Delete "$INSTDIR\\{{this}}"
  {{/each}}

  ; Delete app associations
  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
      !insertmacro APP_UNASSOCIATE "{{ext}}" "{{or association.name ext}}"
    {{/each}}
  {{/each}}

  ; Delete deep links
  {{#each deep_link_protocols as |protocol| ~}}
    ReadRegStr $R7 SHCTX "Software\Classes\\{{protocol}}\shell\open\command" ""
    ${If} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
      DeleteRegKey SHCTX "Software\Classes\\{{protocol}}"
    ${EndIf}
  {{/each}}


  ; Delete uninstaller
  Delete "$INSTDIR\uninstall.exe"

  {{#each resources_ancestors}}
  RMDir /REBOOTOK "$INSTDIR\\{{this}}"
  {{/each}}
  RMDir "$INSTDIR"

  ; Remove shortcuts if not updating
  ${If} $UpdateMode <> 1
    !insertmacro DeleteAppUserModelId

    ; Remove start menu shortcut
    !insertmacro MUI_STARTMENU_GETFOLDER Application $AppStartMenuFolder
    !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      RMDir "$SMPROGRAMS\$AppStartMenuFolder"
    ${EndIf}
    !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}

    ; Remove desktop shortcuts
    !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}

  ; Remove registry information for add/remove programs
  !if "${INSTALLMODE}" == "both"
    DeleteRegKey SHCTX "${UNINSTKEY}"
  !else if "${INSTALLMODE}" == "perMachine"
    DeleteRegKey HKLM "${UNINSTKEY}"
  !else
    DeleteRegKey HKCU "${UNINSTKEY}"
  !endif

  ; Removes the Autostart entry for ${PRODUCTNAME} from the HKCU Run key if it exists.
  ; This ensures the program does not launch automatically after uninstallation if it exists.
  ; If it doesn't exist, it does nothing.
  ; We do this when not updating (to preserve the registry value on updates)
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
  ${EndIf}

  ; Delete app data if the checkbox is selected
  ; and if not updating
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    ; Clear the install location $INSTDIR from registry
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"

    ; Clear the install language from registry
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"

    SetShellVarContext current
    RmDir /r "$APPDATA\${BUNDLEID}"
    RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTUNINSTALL
    !insertmacro NSIS_HOOK_POSTUNINSTALL
  !endif

  ; Auto close if passive mode or updating
  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

Function RestorePreviousInstallLocation
  ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
  StrCmp $4 "" +2 0
    StrCpy $INSTDIR $4
FunctionEnd

Function Skip
  Abort
FunctionEnd

Function SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd
Function SkipInstallModePageIfNeeded
  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    Abort
  ${EndIf}
  ReadRegStr $0 HKLM "${MANUPRODUCTKEY}" ""
  ReadRegStr $1 HKCU "${MANUPRODUCTKEY}" ""
  ReadRegStr $2 HKLM "${UNINSTKEY}" "UninstallString"
  ReadRegStr $3 HKCU "${UNINSTKEY}" "UninstallString"
  ${If} $0 != ""
  ${OrIf} $1 != ""
  ${OrIf} $2 != ""
  ${OrIf} $3 != ""
    Abort
  ${EndIf}
FunctionEnd
Function un.SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd

Function CreateOrUpdateStartMenuShortcut
  ; We used to use product name as MAINBINARYNAME
  ; migrate old shortcuts to target the new MAINBINARYNAME
  StrCpy $R0 0

  !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R0 1
  ${EndIf}

  !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R0 1
  ${EndIf}

  ${If} $R0 = 1
    Return
  ${EndIf}

  ; Skip creating shortcut if in update mode or no shortcut mode
  ; but always create if migrating from wix
  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}

  !if "${STARTMENUFOLDER}" != ""
    CreateDirectory "$SMPROGRAMS\$AppStartMenuFolder"
    CreateShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
  !else
    CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  !endif
FunctionEnd

Function CreateOrUpdateDesktopShortcut
  ; We used to use product name as MAINBINARYNAME
  ; migrate old shortcuts to target the new MAINBINARYNAME
  !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Return
  ${EndIf}

  ; Skip creating shortcut if in update mode or no shortcut mode
  ; but always create if migrating from wix
  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}

  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
FunctionEnd

Function MigrateExistingDesktopShortcut
  ${If} $OldMainBinaryName == ""
  ${OrIf} $OldMainBinaryName == "${MAINBINARYNAME}.exe"
    Return
  ${EndIf}

  !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  ${EndIf}
FunctionEnd
