Unicode true

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"

!define APP_NAME "HLS Downloader"
!define COMPANY_NAME "HLS Downloader"
!ifndef APP_VERSION
!define APP_VERSION "3.0.27"
!endif
!ifndef APP_FILE_VERSION
!define APP_FILE_VERSION "3.0.27.0"
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
Var RemoveDownloads
Var BuildSmoke
Var NativeRegistryArgs
Var PowerShellExe
Var UpgradeBackupDir

!macro InitializePowerShell
  ; makensis emits a 32-bit bootstrapper. On 64-bit Windows, $SYSDIR points to
  ; SysWOW64, whose 32-bit PowerShell cannot read the executable path of the
  ; 64-bit desktop/Core processes. Use the Sysnative bridge so path-scoped
  ; shutdown and 64-bit browser Native Messaging registry writes stay correct.
  StrCpy $PowerShellExe "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"
  ${If} ${RunningX64}
    StrCpy $PowerShellExe "$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe"
  ${EndIf}
!macroend

!define MUI_ABORTWARNING
!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
; MUI2 owns .onUserAbort.  Use its supported hook instead of declaring the
; callback a second time, while still restoring browser registration after a
; cancelled upgrade.
!define MUI_CUSTOMFUNCTION_ABORT RestoreUpgradeAfterAbort

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${STAGE_DIR}\TERMS.txt"
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
  !insertmacro InitializePowerShell
  StrCpy $DeleteSelf "0"
  StrCpy $InstallCompleted "0"
  StrCpy $BuildSmoke "0"
  StrCpy $NativeRegistryArgs ""
  ${GetParameters} $0
  ${GetOptions} $0 "/DELETESELF=" $DeleteSelf
  ${GetOptions} $0 "/BUILD-SMOKE=" $BuildSmoke
  ${If} $BuildSmoke == "1"
    ; Release verification must never replace a developer's real browser
    ; registration while exercising the installer in an isolated directory.
    StrCpy $NativeRegistryArgs '-RegistryPrefix "HKCU:\Software\HLSDownloaderInstallerSmoke"'
  ${EndIf}
FunctionEnd

Function un.onInit
  !insertmacro InitializePowerShell
  StrCpy $BuildSmoke "0"
  StrCpy $NativeRegistryArgs ""
  ${GetParameters} $0
  ${GetOptions} $0 "/BUILD-SMOKE=" $BuildSmoke
  ${If} $BuildSmoke == "1"
    StrCpy $NativeRegistryArgs '-RegistryPrefix "HKCU:\Software\HLSDownloaderInstallerSmoke"'
  ${EndIf}
FunctionEnd

Function ScheduleSelfDelete
  System::Call 'kernel32::GetCurrentProcessId() i .r0'
  ; Never interpolate the installer path into PowerShell source.  A valid
  ; Windows directory may contain a single quote or '$'; the old -Command text
  ; could fail to delete the update or interpret part of that path as code.
  ; Environment values are inherited by the helper without command parsing.
  System::Call 'kernel32::SetEnvironmentVariable(t, t)i("HLS_DOWNLOADER_DELETE_SELF_PATH", "$EXEPATH").r1'
  Exec `"$PowerShellExe" -NoProfile -NonInteractive -WindowStyle Hidden -Command "Wait-Process -Id $0 -ErrorAction SilentlyContinue; Remove-Item -LiteralPath ([Environment]::GetEnvironmentVariable('HLS_DOWNLOADER_DELETE_SELF_PATH','Process')) -Force -ErrorAction SilentlyContinue"`
  System::Call 'kernel32::SetEnvironmentVariable(t, t)i("HLS_DOWNLOADER_DELETE_SELF_PATH", "").r1'
FunctionEnd

Function RestoreBrowserRegistrationAfterAbort
  ${If} $InstallCompleted == "1"
    Return
  ${EndIf}
  ; Installation disconnects every browser before replacing locked files. If
  ; the user aborts or a file operation fails later, restore whichever helper
  ; is still present (old or newly copied) so an interrupted upgrade does not
  ; leave all browser profiles permanently "disconnected".
  IfFileExists "$INSTDIR\scripts\register-native-host.ps1" 0 RestoreBrowserRegistrationAfterAbortDone
  nsExec::ExecToLog '"$PowerShellExe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\scripts\register-native-host.ps1" $NativeRegistryArgs'
RestoreBrowserRegistrationAfterAbortDone:
FunctionEnd

!macro BackupUpgradeFile Name
  IfFileExists "$INSTDIR\${Name}" 0 +2
    Rename "$INSTDIR\${Name}" "$UpgradeBackupDir\${Name}"
!macroend

!macro BackupUpgradeDirectory Name
  IfFileExists "$INSTDIR\${Name}\*.*" 0 +2
    Rename "$INSTDIR\${Name}" "$UpgradeBackupDir\${Name}"
!macroend

!macro RestoreUpgradeFile Name
  IfFileExists "$UpgradeBackupDir\${Name}" 0 +2
    Rename "$UpgradeBackupDir\${Name}" "$INSTDIR\${Name}"
!macroend

!macro RestoreUpgradeDirectory Name
  IfFileExists "$UpgradeBackupDir\${Name}\*.*" 0 +2
    Rename "$UpgradeBackupDir\${Name}" "$INSTDIR\${Name}"
!macroend

Function RestoreApplicationAfterAbort
  ${If} $InstallCompleted == "1"
    Return
  ${EndIf}
  ${If} $UpgradeBackupDir == ""
    Return
  ${EndIf}
  IfFileExists "$UpgradeBackupDir\*.*" 0 RestoreApplicationAfterAbortDone

  Delete "$INSTDIR\HLSDownloader.exe"
  Delete "$INSTDIR\HLSDownloaderCore.exe"
  Delete "$INSTDIR\config.default.json"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\TERMS.md"
  Delete "$INSTDIR\TERMS.txt"
  Delete "$INSTDIR\PRIVACY.md"
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.md"
  Delete "$INSTDIR\sbom.cdx.json"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir /r "$INSTDIR\_internal"
  RMDir /r "$INSTDIR\app"
  RMDir /r "$INSTDIR\runtime"
  RMDir /r "$INSTDIR\bin"
  RMDir /r "$INSTDIR\frontend"
  RMDir /r "$INSTDIR\browser-extension"
  RMDir /r "$INSTDIR\assets"
  RMDir /r "$INSTDIR\scripts"
  Delete "$INSTDIR\native-host\versions\HLSDownloaderNativeHost-${APP_VERSION}.exe"
  Delete "$INSTDIR\native-host\manifests\chrome-${APP_VERSION}.json"
  Delete "$INSTDIR\native-host\manifests\firefox-${APP_VERSION}.json"

  !insertmacro RestoreUpgradeFile "HLSDownloader.exe"
  !insertmacro RestoreUpgradeFile "HLSDownloaderCore.exe"
  !insertmacro RestoreUpgradeFile "config.default.json"
  !insertmacro RestoreUpgradeFile "LICENSE.txt"
  !insertmacro RestoreUpgradeFile "TERMS.md"
  !insertmacro RestoreUpgradeFile "TERMS.txt"
  !insertmacro RestoreUpgradeFile "PRIVACY.md"
  !insertmacro RestoreUpgradeFile "THIRD_PARTY_NOTICES.md"
  !insertmacro RestoreUpgradeFile "sbom.cdx.json"
  !insertmacro RestoreUpgradeFile "Uninstall.exe"
  !insertmacro RestoreUpgradeDirectory "_internal"
  !insertmacro RestoreUpgradeDirectory "app"
  !insertmacro RestoreUpgradeDirectory "runtime"
  !insertmacro RestoreUpgradeDirectory "bin"
  !insertmacro RestoreUpgradeDirectory "frontend"
  !insertmacro RestoreUpgradeDirectory "browser-extension"
  !insertmacro RestoreUpgradeDirectory "assets"
  !insertmacro RestoreUpgradeDirectory "scripts"
  CreateDirectory "$INSTDIR\native-host\versions"
  CreateDirectory "$INSTDIR\native-host\manifests"
  !insertmacro RestoreUpgradeFile "native-host\versions\HLSDownloaderNativeHost-${APP_VERSION}.exe"
  !insertmacro RestoreUpgradeFile "native-host\manifests\chrome-${APP_VERSION}.json"
  !insertmacro RestoreUpgradeFile "native-host\manifests\firefox-${APP_VERSION}.json"
  RMDir /r "$UpgradeBackupDir"
RestoreApplicationAfterAbortDone:
FunctionEnd

Function RestoreUpgradeAfterAbort
  Call RestoreApplicationAfterAbort
  Call RestoreBrowserRegistrationAfterAbort
FunctionEnd

Function .onInstFailed
  Call RestoreUpgradeAfterAbort
FunctionEnd

!macro DisconnectLegacyNativeHost Suffix
  ; An old browser extension can launch its Native Host while this update is
  ; closing the desktop.  Disconnect it first so it cannot immediately create
  ; another old Core and re-lock HLSDownloader.exe mid-upgrade.
  IfFileExists "$INSTDIR\scripts\register-native-host.ps1" 0 DisconnectLegacyNativeHostDone${Suffix}
    DetailPrint "正在暂时断开旧版浏览器连接..."
    nsExec::ExecToLog '"$PowerShellExe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\scripts\register-native-host.ps1" -Unregister $NativeRegistryArgs'
DisconnectLegacyNativeHostDone${Suffix}:
  ; A partial/old install may have lost its helper while registry entries still
  ; let browsers respawn the old host. Remove the known registrations directly
  ; as an idempotent fallback before terminating processes.
  ${If} $BuildSmoke != "1"
    DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\Chromium\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\Vivaldi\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\Opera Software\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\com.ciaooo55.hls_downloader"
  ${EndIf}
!macroend

!macro CloseRunningApp Suffix IncludeNativeHost
  ; Always run the helper.  A damaged/partial old install can have a running
  ; Core even when HLSDownloader.exe has already been removed, and that Core
  ; still locks _internal files that must be replaced by this upgrade.
  ; One bounded helper call is enough: it asks the app to persist resumable
  ; tasks, waits briefly, then force-closes only this install's desktop/Core.
  ; Re-running a 20-second helper four times made a healthy upgrade look hung.
  DetailPrint "正在关闭运行中的 HLS Downloader..."
  nsExec::ExecToStack '"$PowerShellExe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\shutdown-running.ps1" -InstallDir "$INSTDIR" -TimeoutSeconds 12 ${IncludeNativeHost}'
  Pop $R1
  Pop $R2
  ${If} $BuildSmoke == "1"
    FileOpen $R3 "$INSTDIR\installer-smoke-shutdown.log" w
    FileWrite $R3 "exit=$R1$\r$\ninst_dir=$INSTDIR$\r$\nplugin_dir=$PLUGINSDIR$\r$\n$R2"
    FileClose $R3
  ${EndIf}
  ${If} $R1 != 0
    ; Continuing after an unconfirmed shutdown can silently skip locked files
    ; and leave a mixed-version install. Fail within the single bounded helper
    ; window; .onInstFailed restores browser integration for installation.
    DetailPrint "无法安全关闭运行中的程序，安装已停止。"
    SetErrorLevel 2
    Abort
  ${EndIf}
!macroend

Section "Install" SecInstall
  ; nsExec initializes its own plug-in directory on first use. Create it
  ; before extracting the shutdown helper, otherwise the first browser
  ; unregister call can recreate $PLUGINSDIR and delete the helper.
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File /oname=shutdown-running.ps1 "${STAGE_DIR}\scripts\shutdown-running.ps1"
  !insertmacro DisconnectLegacyNativeHost Install
  ; Registration is already removed, so browsers cannot reopen the old Host
  ; during this bounded window. Stop existing Host processes as well: even a
  ; versioned old Host can receive a heartbeat and relaunch the just-closed
  ; desktop/Core, re-locking files while this installer replaces them.
  !insertmacro CloseRunningApp Install "-IncludeNativeHost"
  ; Keep the previous program image as same-volume renames until every new file
  ; and registration step has completed. A failed extraction can then restore
  ; a runnable old version without copying user downloads or task data.
  StrCpy $UpgradeBackupDir "$INSTDIR\.hls-upgrade-backup"
  RMDir /r "$UpgradeBackupDir"
  CreateDirectory "$UpgradeBackupDir"
  !insertmacro BackupUpgradeFile "HLSDownloader.exe"
  !insertmacro BackupUpgradeFile "HLSDownloaderCore.exe"
  !insertmacro BackupUpgradeFile "config.default.json"
  !insertmacro BackupUpgradeFile "LICENSE.txt"
  !insertmacro BackupUpgradeFile "TERMS.md"
  !insertmacro BackupUpgradeFile "TERMS.txt"
  !insertmacro BackupUpgradeFile "PRIVACY.md"
  !insertmacro BackupUpgradeFile "THIRD_PARTY_NOTICES.md"
  !insertmacro BackupUpgradeFile "sbom.cdx.json"
  !insertmacro BackupUpgradeFile "Uninstall.exe"
  !insertmacro BackupUpgradeDirectory "_internal"
  !insertmacro BackupUpgradeDirectory "app"
  !insertmacro BackupUpgradeDirectory "runtime"
  !insertmacro BackupUpgradeDirectory "bin"
  !insertmacro BackupUpgradeDirectory "frontend"
  !insertmacro BackupUpgradeDirectory "browser-extension"
  !insertmacro BackupUpgradeDirectory "assets"
  !insertmacro BackupUpgradeDirectory "scripts"
  CreateDirectory "$UpgradeBackupDir\native-host\versions"
  CreateDirectory "$UpgradeBackupDir\native-host\manifests"
  !insertmacro BackupUpgradeFile "native-host\versions\HLSDownloaderNativeHost-${APP_VERSION}.exe"
  !insertmacro BackupUpgradeFile "native-host\manifests\chrome-${APP_VERSION}.json"
  !insertmacro BackupUpgradeFile "native-host\manifests\firefox-${APP_VERSION}.json"
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
  File "${STAGE_DIR}\LICENSE.txt"
  File "${STAGE_DIR}\TERMS.md"
  File "${STAGE_DIR}\TERMS.txt"
  File "${STAGE_DIR}\PRIVACY.md"
  File "${STAGE_DIR}\THIRD_PARTY_NOTICES.md"
  File "${STAGE_DIR}\sbom.cdx.json"

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
  ; Explorer caches icons by path.  Keep the stable file for application data,
  ; but point shell registrations and recreated shortcuts at a versioned copy
  ; so a cover upgrade cannot keep showing the previous release's icon.
  File /oname=app-icon-${APP_VERSION}.ico "${STAGE_DIR}\assets\app-icon.ico"

  ; Switch the registry to fresh manifest files too.  This avoids racing a
  ; browser which happens to be reading the old manifest while upgrading.
  SetOutPath "$INSTDIR\native-host\manifests"
  File /oname=chrome-${APP_VERSION}.json "${STAGE_DIR}\native-host\chrome.json"
  File /oname=firefox-${APP_VERSION}.json "${STAGE_DIR}\native-host\firefox.json"
  SetOutPath "$INSTDIR\scripts"
  File "${STAGE_DIR}\scripts\register-native-host.ps1"
  File "${STAGE_DIR}\scripts\shutdown-running.ps1"

  DetailPrint "正在切换 Chromium/Firefox 系浏览器连接到新版本..."
  ; Select the host copied by this installer explicitly.  Merely taking the
  ; numerically newest leftover file breaks supported downgrade/reinstall
  ; scenarios and can leave browsers attached to a mixed-version Core.
  nsExec::ExecToStack '"$PowerShellExe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\scripts\register-native-host.ps1" -HostExecutable "$INSTDIR\native-host\versions\HLSDownloaderNativeHost-${APP_VERSION}.exe" $NativeRegistryArgs'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_ICONEXCLAMATION|MB_OK "浏览器连接注册失败，安装完成后可在设置中重新注册。"
  ${EndIf}

  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ${If} $BuildSmoke != "1"
    WriteRegStr HKCU "Software\${APP_NAME}" "InstallDir" "$INSTDIR"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "DisplayName" "${APP_NAME}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "DisplayVersion" "${APP_VERSION}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "Publisher" "${COMPANY_NAME}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}" "DisplayIcon" "$INSTDIR\assets\app-icon-${APP_VERSION}.ico"
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
    WriteRegStr HKCU "Software\Classes\HLSDownloader.Torrent\DefaultIcon" "" "$INSTDIR\assets\app-icon-${APP_VERSION}.ico,0"
    WriteRegStr HKCU "Software\Classes\HLSDownloader.Torrent\shell\open\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'

    ; Context menu only: do not replace the default Internet Shortcut handler.
    WriteRegStr HKCU "Software\Classes\.url\shell\HLSDownload" "" "Download with HLS Downloader"
    WriteRegStr HKCU "Software\Classes\.url\shell\HLSDownload\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'
    WriteRegStr HKCU "Software\Classes\InternetShortcut\shell\HLSDownload" "" "Download with HLS Downloader"
    WriteRegStr HKCU "Software\Classes\InternetShortcut\shell\HLSDownload\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'
    WriteRegStr HKCU "Software\Classes\.magnet\shell\HLSDownload" "" "Download with HLS Downloader"
    WriteRegStr HKCU "Software\Classes\.magnet\shell\HLSDownload\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'
    WriteRegStr HKCU "Software\Classes\.m3u\shell\HLSDownload" "" "Download with HLS Downloader"
    WriteRegStr HKCU "Software\Classes\.m3u\shell\HLSDownload\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'
    WriteRegStr HKCU "Software\Classes\.m3u8\shell\HLSDownload" "" "Download with HLS Downloader"
    WriteRegStr HKCU "Software\Classes\.m3u8\shell\HLSDownload\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'
    WriteRegStr HKCU "Software\Classes\.mpd\shell\HLSDownload" "" "Download with HLS Downloader"
    WriteRegStr HKCU "Software\Classes\.mpd\shell\HLSDownload\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'
    WriteRegStr HKCU "Software\Classes\.html\shell\HLSDownload" "" "Download with HLS Downloader"
    WriteRegStr HKCU "Software\Classes\.html\shell\HLSDownload\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'
    WriteRegStr HKCU "Software\Classes\.htm\shell\HLSDownload" "" "Download with HLS Downloader"
    WriteRegStr HKCU "Software\Classes\.metalink\shell\HLSDownload" "" "Download with HLS Downloader"
    WriteRegStr HKCU "Software\Classes\.meta4\shell\HLSDownload" "" "Download with HLS Downloader"
    WriteRegStr HKCU "Software\Classes\.htm\shell\HLSDownload\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'
    WriteRegStr HKCU "Software\Classes\.metalink\shell\HLSDownload\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'
    WriteRegStr HKCU "Software\Classes\.meta4\shell\HLSDownload\command" "" '$\"$INSTDIR\HLSDownloader.exe$\" $\"%1$\"'

    CreateDirectory "$SMPROGRAMS\${APP_NAME}"
    CreateShortcut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\HLSDownloader.exe" "" "$INSTDIR\assets\app-icon-${APP_VERSION}.ico" 0 SW_SHOWNORMAL "" "Start ${APP_NAME}"
    CreateShortcut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\Uninstall.exe"
    CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\HLSDownloader.exe" "" "$INSTDIR\assets\app-icon-${APP_VERSION}.ico" 0 SW_SHOWNORMAL "" "Start ${APP_NAME}"
  ${EndIf}
  StrCpy $InstallCompleted "1"
  RMDir /r "$UpgradeBackupDir"
  ${If} $DeleteSelf == "1"
    Call ScheduleSelfDelete
  ${EndIf}
SectionEnd

Section "Uninstall"
  InitPluginsDir
  CopyFiles /SILENT "$INSTDIR\scripts\shutdown-running.ps1" "$PLUGINSDIR\shutdown-running.ps1"
  ; Prevent a browser extension from reopening its host while the uninstall is
  ; removing versioned host files.  Updates intentionally do not do this.
  nsExec::ExecToLog '"$PowerShellExe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\scripts\register-native-host.ps1" -Unregister $NativeRegistryArgs'
  !insertmacro CloseRunningApp Uninstall "-IncludeNativeHost"
  nsExec::ExecToLog '"$PowerShellExe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\shutdown-running.ps1" -InstallDir "$INSTDIR" -TimeoutSeconds 5 -IncludeNativeHost'

  StrCpy $RemoveDownloads "preserve"
  IfSilent RemoveApplicationData
  MessageBox MB_ICONQUESTION|MB_YESNO|MB_DEFBUTTON2 "是否同时删除已下载的视频？$\r$\n$\r$\n选择“否”只删除程序、设置、任务历史和缓存。" IDNO RemoveApplicationData
  StrCpy $RemoveDownloads "delete"
  RMDir /r "$PROFILE\Downloads\HLS Downloader"
  RMDir /r "$INSTDIR\downloads"

RemoveApplicationData:
  ; Process files live here by default and must never keep the install directory behind.
  RMDir /r "$INSTDIR\.tasks"
  ${If} $BuildSmoke != "1"
    Delete "$DESKTOP\${APP_NAME}.lnk"
    Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
    Delete "$SMPROGRAMS\${APP_NAME}\Uninstall ${APP_NAME}.lnk"
    Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
    RMDir "$SMPROGRAMS\${APP_NAME}"
  ${EndIf}

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

  ${If} $BuildSmoke != "1"
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
    DeleteRegKey HKCU "Software\Classes\.url\shell\HLSDownload"
    DeleteRegKey HKCU "Software\Classes\InternetShortcut\shell\HLSDownload"
    DeleteRegKey HKCU "Software\Classes\.magnet\shell\HLSDownload"
    DeleteRegKey HKCU "Software\Classes\.m3u\shell\HLSDownload"
    DeleteRegKey HKCU "Software\Classes\.m3u8\shell\HLSDownload"
    DeleteRegKey HKCU "Software\Classes\.mpd\shell\HLSDownload"
    DeleteRegKey HKCU "Software\Classes\.html\shell\HLSDownload"
    DeleteRegKey HKCU "Software\Classes\.htm\shell\HLSDownload"
    DeleteRegKey HKCU "Software\Classes\.metalink\shell\HLSDownload"
    DeleteRegKey HKCU "Software\Classes\.meta4\shell\HLSDownload"
    DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\Chromium\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\Vivaldi\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\Opera Software\NativeMessagingHosts\com.ciaooo55.hls_downloader"
    DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\com.ciaooo55.hls_downloader"
  ${EndIf}

  ${If} $RemoveDownloads == "delete"
    RMDir /r "$INSTDIR"
  ${Else}
    RMDir "$INSTDIR"
  ${EndIf}

  ${If} $BuildSmoke != "1"
    ; The core may release its database and cache files just after the UI exits.
    RMDir /r "$LOCALAPPDATA\HLS Downloader"
    Sleep 1000
    RMDir /r "$LOCALAPPDATA\HLS Downloader"
    Sleep 1000
    RMDir /r "$LOCALAPPDATA\HLS Downloader"
  ${EndIf}

  Delete "$INSTDIR\HLSDownloader.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Sleep 1000
  Delete "$INSTDIR\HLSDownloader.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
SectionEnd
