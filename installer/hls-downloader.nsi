Unicode true

!include "MUI2.nsh"
!include "LogicLib.nsh"

!define APP_NAME "HLS Downloader"
!define COMPANY_NAME "HLS Downloader"
!ifndef APP_VERSION
  !define APP_VERSION "1.1.1"
!endif
!define WEBVIEW2_GUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

!ifndef STAGE_DIR
  !error "STAGE_DIR is required. Pass /DSTAGE_DIR=<path> to makensis."
!endif

!ifndef OUT_FILE
  !define OUT_FILE "HLSDownloaderSetup.exe"
!endif

Name "${APP_NAME}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\HLS Downloader"
InstallDirRegKey HKCU "Software\${APP_NAME}" "InstallDir"
RequestExecutionLevel user

!define MUI_ABORTWARNING

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

Section "Install" SecInstall
  SetOutPath "$INSTDIR"

  File "${STAGE_DIR}\HLSDownloader.exe"
  File "${STAGE_DIR}\MicrosoftEdgeWebview2Setup.exe"
  File /oname=config.default.json "${STAGE_DIR}\config.json"

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
SectionEnd

Section "Uninstall"
  DetailPrint "正在关闭 HLS Downloader..."
  IfFileExists "$INSTDIR\HLSDownloader.exe" 0 ShutdownDone
    ExecWait '$\"$INSTDIR\HLSDownloader.exe$\" --shutdown'
    Sleep 2000
    nsExec::ExecToStack 'taskkill /IM HLSDownloader.exe /F'
    Pop $1
    Pop $2
ShutdownDone:

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
  Delete "$INSTDIR\MicrosoftEdgeWebview2Setup.exe"
  Delete "$INSTDIR\config.default.json"
  Delete "$INSTDIR\config.json"
  Delete "$INSTDIR\data.db"
  Delete "$INSTDIR\data.db-shm"
  Delete "$INSTDIR\data.db-wal"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir /r "$INSTDIR\frontend"
  RMDir /r "$INSTDIR\userscript"
  RMDir /r "$INSTDIR\bin"
  RMDir /r "$INSTDIR\.webview"
  RMDir /r "$INSTDIR\.data"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
  DeleteRegKey HKCU "Software\${APP_NAME}"

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
