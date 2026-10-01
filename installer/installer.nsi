; Data Traffic Manager - NSIS installer
;
; Build (from the repository root, after `cargo build --release`):
;   mkdir dist
;   makensis /DVERSION=0.1.0 installer\installer.nsi
;
; Optional defines:
;   /DSOURCE_EXE=<path>   executable to package (default: target\release\data-traffic-manager.exe)
;   /DOUTPUT_DIR=<path>   output directory (default: dist)
;
; Command line of the generated installer:
;   /S                    silent install
;   /D=<dir>              installation directory (must be the last argument)

Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma

!ifndef VERSION
  !error "Pass the version, e.g. makensis /DVERSION=0.1.0 installer\installer.nsi"
!endif
!ifndef SOURCE_EXE
  !define SOURCE_EXE "..\target\release\data-traffic-manager.exe"
!endif
!ifndef OUTPUT_DIR
  !define OUTPUT_DIR "..\dist"
!endif

!define APP_NAME "Data Traffic Manager"
!define APP_EXE "data-traffic-manager.exe"
!define APP_ID "DataTrafficManager"
!define PUBLISHER "SHIN DATA CENTER"
!define WEBSITE "https://github.com/SHIN-DATA-CENTER/Data-Traffic-Manager"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}"
; Value written by the application when "start at sign-in" is enabled.
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define RUN_VALUE "DataTrafficManager"
; Folder under %LOCALAPPDATA% holding settings and usage history.
!define DATA_DIR_NAME "Data-Traffic-Manager"

Name "${APP_NAME}"
OutFile "${OUTPUT_DIR}\DataTrafficManager-${VERSION}-setup-x64.exe"
InstallDir "$PROGRAMFILES64\${APP_NAME}"
RequestExecutionLevel admin
BrandingText "${APP_NAME} ${VERSION}"
ShowInstDetails show
ShowUninstDetails show

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"

; --- Pages -------------------------------------------------------------------

!define MUI_ICON "..\assets\icon.ico"
!define MUI_UNICON "..\assets\icon.ico"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_SMALLDESC

!define MUI_WELCOMEPAGE_TEXT "${APP_NAME} ${VERSION} をインストールします。$\r$\n$\r$\nネットワークアダプターごとの通信速度と通信量を監視するアプリケーションです。WireGuard などの VPN アダプターにも対応しています。$\r$\n$\r$\nアプリケーションが起動している場合は、インストール時に自動で終了します（記録中の通信量は保存されます）。$\r$\n$\r$\n[次へ] をクリックして続行してください。"

!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "${APP_NAME} を起動する"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchApplication

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "Japanese"

; --- Version information of the installer executable -------------------------

VIProductVersion "${VERSION}.0"
VIFileVersion "${VERSION}.0"
VIAddVersionKey /LANG=${LANG_JAPANESE} "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=${LANG_JAPANESE} "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_JAPANESE} "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_JAPANESE} "FileDescription" "${APP_NAME} セットアップ"
VIAddVersionKey /LANG=${LANG_JAPANESE} "CompanyName" "${PUBLISHER}"
VIAddVersionKey /LANG=${LANG_JAPANESE} "LegalCopyright" "Copyright (c) 2026 ${PUBLISHER}"

; --- Helpers shared by the installer and the uninstaller ---------------------

; Ends a running instance before its executable is replaced or removed.
; The application is first asked to save its data and exit (`--quit`); if it
; is still running after 10 seconds (or runs in another user's session) it is
; terminated. Usage is not lost either way: the next start books the traffic
; since the last save from the adapter counters.
!macro DEFINE_CLOSE_APPLICATION PREFIX
Function ${PREFIX}CloseApplication
  ${If} ${FileExists} "$INSTDIR\${APP_EXE}"
    DetailPrint "起動中の ${APP_NAME} を終了しています..."
    ExecWait '"$INSTDIR\${APP_EXE}" --quit'
  ${EndIf}
  StrCpy $R0 0
  ${Do}
    ; With /FO CSV a matching process is listed as a line starting with a quote.
    nsExec::ExecToStack 'tasklist /FI "IMAGENAME eq ${APP_EXE}" /FO CSV /NH'
    Pop $R1 ; exit code
    Pop $R2 ; output
    StrCpy $R2 $R2 1
    ${If} $R2 != '"'
      ${Break}
    ${EndIf}
    ${If} $R0 >= 20
      DetailPrint "応答がないため ${APP_NAME} を強制終了します"
      nsExec::Exec 'taskkill /F /IM ${APP_EXE}'
      Pop $R1
      Sleep 1000
      ${Break}
    ${EndIf}
    Sleep 500
    IntOp $R0 $R0 + 1
  ${Loop}
FunctionEnd
!macroend

!insertmacro DEFINE_CLOSE_APPLICATION ""
!insertmacro DEFINE_CLOSE_APPLICATION "un."

; --- Installer ----------------------------------------------------------------

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "${APP_NAME} は 64 ビット版の Windows が必要です。" /SD IDOK
    Abort
  ${EndIf}
  SetRegView 64
  SetShellVarContext all
  ; Upgrade in place when a previous version is installed.
  ReadRegStr $0 HKLM "${UNINST_KEY}" "InstallLocation"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
  ${EndIf}
FunctionEnd

Section "${APP_NAME}（必須）" SecMain
  SectionIn RO
  Call CloseApplication

  SetOutPath "$INSTDIR"
  File "/oname=${APP_EXE}" "${SOURCE_EXE}"
  File "/oname=LICENSE.txt" "..\LICENSE"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0 SW_SHOWNORMAL "" "ネットワークアダプターの通信量モニター"

  ; Entry in "Apps" / "Programs and Features".
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE},0"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKLM "${UNINST_KEY}" "URLInfoAbout" "${WEBSITE}"
  WriteRegStr HKLM "${UNINST_KEY}" "HelpLink" "${WEBSITE}"
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKLM "${UNINST_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "デスクトップにショートカットを作成" SecDesktop
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "アプリケーション本体とスタートメニューのショートカット"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "デスクトップに ${APP_NAME} のショートカットを作成します"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; Starts the application without administrator rights: the installer itself
; runs elevated, and Explorer launches programs as the signed-in user.
Function LaunchApplication
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\${APP_EXE}"'
FunctionEnd

; --- Uninstaller --------------------------------------------------------------

Function un.onInit
  SetRegView 64
  SetShellVarContext all
FunctionEnd

Section "Uninstall"
  Call un.CloseApplication

  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APP_NAME}.lnk"
  Delete "$DESKTOP\${APP_NAME}.lnk"
  DeleteRegKey HKLM "${UNINST_KEY}"

  ; "Start at sign-in" entry created by the application for this user.
  DeleteRegValue HKCU "${RUN_KEY}" "${RUN_VALUE}"

  ; Usage history and settings are kept unless the user asks otherwise
  ; (always kept for a silent uninstall).
  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "記録した使用量の履歴と設定も削除しますか？$\r$\n$\r$\n[いいえ] を選ぶと、再インストールしたときに履歴を引き継げます。" /SD IDNO IDNO keep_data
    SetShellVarContext current
    RMDir /r "$LOCALAPPDATA\${DATA_DIR_NAME}"
    SetShellVarContext all
  keep_data:
SectionEnd
