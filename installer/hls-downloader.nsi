Unicode true

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!define APP_NAME "HLS Downloader"
!define COMPANY_NAME "HLS Downloader"
!ifndef APP_VERSION
  !define APP_VERSION "1.2.6"
!endif
!define WEBVIEW2_GUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

!ifndef STAGE_DIR
  !error "STAGE_DIR is required. Pass /DSTAGE_DIR=<path> to makensis."
!endif

!ifndef OUT_FILE
  !define OUT_FILE "HLSDownloaderSetup.exe"
!endif

!ifndef ICON_FILE
  !error "ICON_FILE is required. Pass /DICON_FILE=<path> to makensis."
!endif

Name "${APP_NAME}"
OutFile "${OUT_FILE}"
Icon "${ICON_FILE}"
UninstallIcon "${ICON_FILE}"
InstallDir "$LOCALAPPDATA\Programs\HLS Downloader"
InstallDirRegKey HKCU "Software\${APP_NAME}" "InstallDir"
RequestExecutionLevel user

Var DeleteSelf
Var InstallCompleted

!define MUI_ABORTWARNING
!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\HLSDownloader.exe"
!define MUI_FINISHPAGE_RUN_TEXT "运行 HLS Downloader"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "SimpChinese"

Function .onInit
  StrCpy $DeleteSelf "0"
  StrCpy $InstallCompleted "0"
  ${GetParameters} $0
  ${GetOptions} $0 "/DELETESELF=" $DeleteSelf
FunctionEnd

Function ScheduleSelfDelete
  System::Call 'kernel32::GetCurrentProcessId() i .r0'
  Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -WindowStyle Hidden -Command "Wait-Process -Id $0 -ErrorAction SilentlyContinue; Remove-Item -LiteralPath '$EXEPATH' -Force -ErrorAction SilentlyContinue"`
FunctionEnd

!macro CloseRunningApp Suffix
  IfFileExists "$INSTDIR\HLSDownloader.exe" 0 CloseRunningAppDone${Suffix}
    DetailPrint "正在关闭运行中的 HLS Downloader..."
    nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\shutdown-running.ps1"'
    Pop $0
    Pop $1
CloseRunningAppForce${Suffix}:
    nsExec::ExecToStack 'taskkill /IM HLSDownloader.exe /F'
    Pop $0
    Pop $1
    Sleep 500
CloseRunningAppDone${Suffix}:
!macroend

Section "Install" SecInstall
  SetOutPath "$PLUGINSDIR"
  File /oname=shutdown-running.ps1 "${STAGE_DIR}\scripts\shutdown-running.ps1"
  !insertmacro CloseRunningApp Install
  ; A clean runtime directory prevents modules left by an older onedir build
  ; from shadowing files in the new package.
  RMDir /r "$INSTDIR\_internal"
  SetOutPath "$INSTDIR"

  File "${STAGE_DIR}\HLSDownloader.exe"
  File "${STAGE_DIR}\HLSDownloaderNativeHost.exe"
  File "${STAGE_DIR}\MicrosoftEdgeWebview2Setup.exe"
  File /oname=config.default.json "${STAGE_DIR}\config.json"

  SetOutPath "$INSTDIR\_internal"
  File /r "${STAGE_DIR}\_internal\*"

  SetRegView 32
  ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  ${If} $0 == ""
    ReadRegStr $0 HKCU "Software\Microsoft\EdgeUpdate\Clients\${WEBVIEW2_GUID}" "pv"
  ${EndIf}
  ${If} $0 == ""
    DetailPrint "正在安装 Microsoft Edge WebView2 Runtime..."
    ExecWait '"$INSTDIR\MicrosoftEdgeWebview2Setup.exe" /silent /install' $1
    ${If} $1 != 0
      MessageBox MB_ICONSTOP|MB_OK "WebView2 Runtime 安装失败（错误码 $1）。请检查网络后重新运行安装程序。"
      Abort
    ${EndIf}
  ${EndIf}
  Delete "$INSTDIR\MicrosoftEdgeWebview2Setup.exe"

  SetOutPath "$INSTDIR\bin"
  File "${STAGE_DIR}\bin\ffmpeg.exe"
  File "${STAGE_DIR}\bin\ffprobe.exe"

  SetOutPath "$INSTDIR\frontend"
  File /r "${STAGE_DIR}\frontend\dist"

  SetOutPath "$INSTDIR\userscript"
  File "${STAGE_DIR}\userscript\m3u8-sniffer.user.js"

  SetOutPath "$INSTDIR\assets"
  File "${STAGE_DIR}\assets\app-icon.png"
  File "${STAGE_DIR}\assets\app-icon.ico"

  SetOutPath "$INSTDIR\native-host"
  File "${STAGE_DIR}\native-host\chrome.json"
  File "${STAGE_DIR}\native-host\firefox.json"
  SetOutPath "$INSTDIR\scripts"
  File "${STAGE_DIR}\scripts\register-native-host.ps1"
  File "${STAGE_DIR}\scripts\shutdown-running.ps1"

  DetailPrint "正在注册 Chrome/Edge/Firefox 浏览器连接..."
  nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\scripts\register-native-host.ps1"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_ICONEXCLAMATION|MB_OK "浏览器连接注册失败，安装完成后可在设置中重新注册。"
  ${EndIf}

  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  WriteRegStr HKCU "Software\${APP_NAME}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "Publisher" "${COMPANY_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "DisplayIcon" "$INSTDIR\HLSDownloader.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "UninstallString" '$\"$INSTDIR\Uninstall.exe$\"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "QuietUninstallString" '$\"$INSTDIR\Uninstall.exe$\" /S'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\HLSDownloader.exe" "" "$INSTDIR\HLSDownloader.exe" 0 SW_SHOWNORMAL "" "Start ${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\Uninstall.exe"
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\HLSDownloader.exe" "" "$INSTDIR\HLSDownloader.exe" 0 SW_SHOWNORMAL "" "Start ${APP_NAME}"
  StrCpy $InstallCompleted "1"
  ${If} $DeleteSelf == "1"
    Call ScheduleSelfDelete
  ${EndIf}
SectionEnd

Section "Uninstall"
  InitPluginsDir
  CopyFiles /SILENT "$INSTDIR\scripts\shutdown-running.ps1" "$PLUGINSDIR\shutdown-running.ps1"
  !insertmacro CloseRunningApp Uninstall

  StrCpy $0 "preserve"
  IfSilent RemoveApplicationData
  MessageBox MB_ICONQUESTION|MB_YESNO|MB_DEFBUTTON2 "是否同时删除已下载的视频？$\r$\n$\r$\n选择“否”只删除程序、设置、任务历史和缓存。" IDNO RemoveApplicationData
  StrCpy $0 "delete"
  RMDir /r "$PROFILE\Downloads\HLS Downloader"
  RMDir /r "$INSTDIR\downloads"

RemoveApplicationData:
  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\Uninstall ${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"

  Delete "$INSTDIR\HLSDownloader.exe"
  Delete "$INSTDIR\HLSDownloaderNativeHost.exe"
  Delete "$INSTDIR\MicrosoftEdgeWebview2Setup.exe"
  Delete "$INSTDIR\config.default.json"
  Delete "$INSTDIR\config.json"
  Delete "$INSTDIR\data.db"
  Delete "$INSTDIR\data.db-shm"
  Delete "$INSTDIR\data.db-wal"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir /r "$INSTDIR\frontend"
  RMDir /r "$INSTDIR\userscript"
  RMDir /r "$INSTDIR\assets"
  RMDir /r "$INSTDIR\native-host"
  RMDir /r "$INSTDIR\scripts"
  RMDir /r "$INSTDIR\bin"
  RMDir /r "$INSTDIR\_internal"
  RMDir /r "$INSTDIR\.webview"
  RMDir /r "$INSTDIR\.data"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
  DeleteRegKey HKCU "Software\${APP_NAME}"
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.ciaooo55.hls_downloader"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.ciaooo55.hls_downloader"
  DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\com.ciaooo55.hls_downloader"

  ${If} $0 == "delete"
    RMDir /r "$INSTDIR"
  ${Else}
    RMDir "$INSTDIR"
  ${EndIf}

  ; WebView2 helpers can release cache files just after the main process exits.
  RMDir /r "$LOCALAPPDATA\HLS Downloader"
  Sleep 1000
  RMDir /r "$LOCALAPPDATA\HLS Downloader"
  Sleep 1000
  RMDir /r "$LOCALAPPDATA\HLS Downloader"

  Delete "$INSTDIR\HLSDownloader.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Sleep 1000
  Delete "$INSTDIR\HLSDownloader.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
SectionEnd
