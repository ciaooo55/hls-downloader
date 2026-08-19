Unicode true

!include "MUI2.nsh"
!include "x64.nsh"

!define APP_NAME "HLS Downloader"
!ifndef APP_VERSION
!define APP_VERSION "6.0.0-dev"
!endif

!ifndef STAGE_DIR
  !error "STAGE_DIR is required. Pass /DSTAGE_DIR=<path> to makensis."
!endif
!ifndef OUT_FILE
  !define OUT_FILE "HLSDownloader-v6-Setup.exe"
!endif
!ifndef ICON_FILE
  !error "ICON_FILE is required. Pass /DICON_FILE=<path> to makensis."
!endif

Name "${APP_NAME}"
OutFile "${OUT_FILE}"
Icon "${ICON_FILE}"
UninstallIcon "${ICON_FILE}"
InstallDir "$LOCALAPPDATA\Programs\HLS Downloader v6"
InstallDirRegKey HKCU "Software\${APP_NAME} v6" "InstallDir"
RequestExecutionLevel user

Var PowerShellExe

!macro InitializePowerShell
  StrCpy $PowerShellExe "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"
  ${If} ${RunningX64}
    StrCpy $PowerShellExe "$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe"
  ${EndIf}
!macroend

!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
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
FunctionEnd

Function un.onInit
  !insertmacro InitializePowerShell
FunctionEnd

Section "Install"
  SetOutPath "$INSTDIR"
  File "${STAGE_DIR}\HLSDownloader.exe"
  File "${STAGE_DIR}\HLSDownloaderNativeHost.exe"
  File "${STAGE_DIR}\ffmpeg.exe"
  File "${STAGE_DIR}\ffprobe.exe"
  File /nonfatal "${STAGE_DIR}\libmpv-2.dll"
  File /nonfatal "${STAGE_DIR}\curl-impersonate.exe"
  File /nonfatal "${STAGE_DIR}\curl_chrome131.exe"
  File /nonfatal "${STAGE_DIR}\curl-impersonate-chrome.exe"
  File "${STAGE_DIR}\README.txt"
  File /nonfatal "${STAGE_DIR}\TERMS.txt"
  SetOutPath "$INSTDIR\native-host"
  File "${STAGE_DIR}\native-host\chrome.json"
  File "${STAGE_DIR}\native-host\firefox.json"
  File "${STAGE_DIR}\native-host\v6-chrome.json"
  File "${STAGE_DIR}\native-host\v6-firefox.json"
  SetOutPath "$INSTDIR\scripts"
  File "${STAGE_DIR}\scripts\register-native-host.ps1"
  CreateShortCut "$SMPROGRAMS\HLS Downloader v6.lnk" "$INSTDIR\HLSDownloader.exe"
  WriteRegStr HKCU "Software\${APP_NAME} v6" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  nsExec::ExecToLog '"$PowerShellExe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\scripts\register-native-host.ps1" -Cutover -HostExecutable "$INSTDIR\HLSDownloaderNativeHost.exe"'
SectionEnd

Section "Uninstall"
  nsExec::ExecToLog '"$PowerShellExe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\scripts\register-native-host.ps1" -Cutover -Unregister'
  Delete "$SMPROGRAMS\HLS Downloader v6.lnk"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "Software\${APP_NAME} v6"
SectionEnd
