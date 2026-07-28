Unicode true

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!define APP_NAME "HLS Downloader"
!define COMPANY_NAME "HLS Downloader"
!ifndef APP_VERSION
!define APP_VERSION "1.6.22"
!endif
!ifndef APP_FILE_VERSION
!define APP_FILE_VERSION "1.6.22.0"
!endif

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
VIProductVersion "${APP_FILE_VERSION}"
VIAddVersionKey /LANG=1033 "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "FileVersion" "${APP_FILE_VERSION}"
VIAddVersionKey /LANG=1033 "FileDescription" "${APP_NAME} Windows installer"
VIAddVersionKey /LANG=1033 "CompanyName" "${COMPANY_NAME}"

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

!macro DisconnectLegacyNativeHost Suffix
  ; An old browser extension can launch its Native Host while this update is
  ; closing the desktop.  Disconnect it first so it cannot immediately create
  ; another old Core and re-lock HLSDownloader.exe mid-upgrade.
  IfFileExists "$INSTDIR\scripts\register-native-host.ps1" 0 DisconnectLegacyNativeHostDone${Suffix}
    DetailPrint "正在暂时断开旧版浏览器连接..."
    nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\scripts\register-native-host.ps1" -Unregister'
DisconnectLegacyNativeHostDone${Suffix}:
!macroend

!macro CloseRunningApp Suffix IncludeNativeHost
  IfFileExists "$INSTDIR\HLSDownloader.exe" 0 CloseRunningAppDone${Suffix}
  StrCpy $R0 0
CloseRunningAppRetry${Suffix}:
    IntOp $R0 $R0 + 1
    DetailPrint "正在关闭运行中的 HLS Downloader..."
    nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\shutdown-running.ps1" -InstallDir "$INSTDIR" -TimeoutSeconds 20 ${IncludeNativeHost}'
    Pop $R1
    Pop $R2
    ${If} $R1 != 0
      ; Antivirus and Explorer can hold a just-closed executable briefly.
      ; Retry automatically after the Host/desktop process tree is gone so a
      ; normal overwrite never requires users to race a dialog manually.
      ${If} $R0 < 4
        Sleep 1000
        Goto CloseRunningAppRetry${Suffix}
      ${EndIf}
      ; The helper has already forced the desktop/Core/Host process tree down.
      ; On some Windows builds nsExec can report a bridge error after that
      ; cleanup has succeeded.  Do not turn that stale return value into a
      ; blocking Retry/Cancel dialog: NSIS performs the actual write check
      ; when it replaces the executable a few lines below.
      DetailPrint "关闭结果未确认，继续验证并覆盖程序文件..."
    ${EndIf}
    Goto CloseRunningAppDone${Suffix}
CloseRunningAppDone${Suffix}:
!macroend

Section "Install" SecInstall
  SetOutPath "$PLUGINSDIR"
  File /oname=shutdown-running.ps1 "${STAGE_DIR}\scripts\shutdown-running.ps1"
  !insertmacro DisconnectLegacyNativeHost Install
  !insertmacro CloseRunningApp Install "-IncludeNativeHost"
  ; Remove both generations of the old desktop shell before writing Tauri.
  ; v1.4.0 shipped a Kotlin/Compose image in app/ and runtime/.  Leaving it
  ; behind made a half-updated install easy to launch from a stale shortcut.
  ; The current shell is the single HLSDownloader.exe in $INSTDIR.
  RMDir /r "$INSTDIR\_internal"
  RMDir /r "$INSTDIR\app"
  RMDir /r "$INSTDIR\runtime"
  SetOutPath "$INSTDIR"

  File "${STAGE_DIR}\HLSDownloader.exe"
  File "${STAGE_DIR}\HLSDownloaderCore.exe"
  ; Native Messaging processes are launched by Chrome, Edge and Firefox and
  ; may legitimately remain alive after the desktop app has exited.  Never
  ; overwrite their executable in place: a versioned target lets an existing
  ; browser connection finish while newly created connections use this build.
  SetOutPath "$INSTDIR\native-host\versions"
  File /oname=HLSDownloaderNativeHost-${APP_VERSION}.exe "${STAGE_DIR}\HLSDownloaderNativeHost.exe"
  SetOutPath "$INSTDIR"
  File /oname=config.default.json "${STAGE_DIR}\config.json"

  SetOutPath "$INSTDIR\_internal"
  File /r "${STAGE_DIR}\_internal\*"

  SetOutPath "$INSTDIR\bin"
  File "${STAGE_DIR}\bin\ffmpeg.exe"
  File "${STAGE_DIR}\bin\ffprobe.exe"

  SetOutPath "$INSTDIR\frontend"
  File /r "${STAGE_DIR}\frontend\dist"

  SetOutPath "$INSTDIR\browser-extension\chrome"
  File /r "${STAGE_DIR}\browser-extension\chrome\*"

  SetOutPath "$INSTDIR\assets"
  File "${STAGE_DIR}\assets\app-icon.png"
  File "${STAGE_DIR}\assets\app-icon.ico"

  ; Switch the registry to fresh manifest files too.  This avoids racing a
  ; browser which happens to be reading the old manifest while upgrading.
  SetOutPath "$INSTDIR\native-host\manifests"
  File /oname=chrome-${APP_VERSION}.json "${STAGE_DIR}\native-host\chrome.json"
  File /oname=firefox-${APP_VERSION}.json "${STAGE_DIR}\native-host\firefox.json"
  SetOutPath "$INSTDIR\scripts"
  File "${STAGE_DIR}\scripts\register-native-host.ps1"
  File "${STAGE_DIR}\scripts\shutdown-running.ps1"

  DetailPrint "正在切换 Chrome/Edge/Firefox 浏览器连接到新版本..."
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

  ; Preserve the user's existing BT client. Reinstalling our own association
  ; keeps the original value stored during the first install.
  ReadRegStr $0 HKCU "Software\Classes\.torrent" ""
  ${If} $0 != "HLSDownloader.Torrent"
    ${If} $0 == ""
      WriteRegStr HKCU "Software\${APP_NAME}" "PreviousTorrentProgId" "__none__"
    ${Else}
      WriteRegStr HKCU "Software\${APP_NAME}" "PreviousTorrentProgId" "$0"
    ${EndIf}
  ${EndIf}
  WriteRegStr HKCU "Software\Classes\.torrent" "" "HLSDownloader.Torrent"
  WriteRegStr HKCU "Software\Classes\HLSDownloader.Torrent" "" "BT 种子文件"
  WriteRegStr HKCU "Software\Classes\HLSDownloader.Torrent\DefaultIcon" "" "$INSTDIR\HLSDownloader.exe,0"
  WriteRegStr HKCU "Software\Classes\HLSDownloader.Torrent\shell\open\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'

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
  ; Prevent a browser extension from reopening its host while the uninstall is
  ; removing versioned host files.  Updates intentionally do not do this.
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\scripts\register-native-host.ps1" -Unregister'
  !insertmacro CloseRunningApp Uninstall "-IncludeNativeHost"
  nsExec::ExecToLog '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\shutdown-running.ps1" -InstallDir "$INSTDIR" -TimeoutSeconds 5 -IncludeNativeHost'

  StrCpy $0 "preserve"
  IfSilent RemoveApplicationData
  MessageBox MB_ICONQUESTION|MB_YESNO|MB_DEFBUTTON2 "是否同时删除已下载的视频？$\r$\n$\r$\n选择“否”只删除程序、设置、任务历史和缓存。" IDNO RemoveApplicationData
  StrCpy $0 "delete"
  RMDir /r "$PROFILE\Downloads\HLS Downloader"
  RMDir /r "$INSTDIR\downloads"

RemoveApplicationData:
  ; Process files live here by default and must never keep the install directory behind.
  RMDir /r "$INSTDIR\.tasks"
  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\Uninstall ${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"

  Delete "$INSTDIR\HLSDownloader.exe"
  Delete "$INSTDIR\HLSDownloaderCore.exe"
  Delete "$INSTDIR\HLSDownloaderNativeHost.exe"
  Delete "$INSTDIR\config.default.json"
  Delete "$INSTDIR\config.json"
  Delete "$INSTDIR\data.db"
  Delete "$INSTDIR\data.db-shm"
  Delete "$INSTDIR\data.db-wal"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir /r "$INSTDIR\frontend"
  RMDir /r "$INSTDIR\browser-extension"
  RMDir /r "$INSTDIR\assets"
  RMDir /r "$INSTDIR\native-host"
  RMDir /r "$INSTDIR\scripts"
  RMDir /r "$INSTDIR\bin"
  RMDir /r "$INSTDIR\_internal"
  RMDir /r "$INSTDIR\app"
  RMDir /r "$INSTDIR\runtime"
  RMDir /r "$INSTDIR\.data"

  ReadRegStr $0 HKCU "Software\Classes\.torrent" ""
  ${If} $0 == "HLSDownloader.Torrent"
    ReadRegStr $1 HKCU "Software\${APP_NAME}" "PreviousTorrentProgId"
    ${If} $1 == "__none__"
      DeleteRegValue HKCU "Software\Classes\.torrent" ""
    ${ElseIf} $1 != ""
      WriteRegStr HKCU "Software\Classes\.torrent" "" "$1"
    ${Else}
      DeleteRegValue HKCU "Software\Classes\.torrent" ""
    ${EndIf}
  ${EndIf}
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
  DeleteRegKey HKCU "Software\${APP_NAME}"
  DeleteRegKey HKCU "Software\Classes\HLSDownloader.Torrent"
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.ciaooo55.hls_downloader"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.ciaooo55.hls_downloader"
  DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\com.ciaooo55.hls_downloader"

  ${If} $0 == "delete"
    RMDir /r "$INSTDIR"
  ${Else}
    RMDir "$INSTDIR"
  ${EndIf}

  ; The core may release its database and cache files just after the UI exits.
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
