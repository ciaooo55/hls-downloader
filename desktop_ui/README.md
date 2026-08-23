# HLS Downloader Desktop 7.0.0

Compose Desktop workbench for HLS Downloader. This module is the 7.x desktop interface; `HLSDownloaderEngine.exe` owns all download state and SQLite through the existing framed IPC protocol.

```powershell
$env:GRADLE_USER_HOME = 'E:\HLSDownloaderBuildCache\gradle'
$env:JAVA_HOME = 'E:\HLSDownloaderBuildCache\jdk-21'
.\gradlew.bat run
```