# HLS Downloader Desktop 7.0.2

Compose Desktop workbench for HLS Downloader. This module is the 7.x desktop interface; `HLSDownloaderEngine.exe` owns all download state and SQLite through the existing framed IPC protocol.

```powershell
$env:GRADLE_USER_HOME = 'E:\HLSDownloaderBuildCache\gradle'
$env:JAVA_HOME = 'E:\HLSDownloaderBuildCache\jdk-21'
.\gradlew.bat run
```

The packaged product defaults to Skiko software rendering for broad Windows compatibility. For diagnostics or performance comparison, opt into a different backend without editing source:

```powershell
$env:HLS_UI_RENDER_API = 'ANGLE'
.\gradlew.bat run

$env:HLS_UI_RENDER_API = 'DIRECT3D'
.\gradlew.bat run

# Equivalent Gradle property form
.\gradlew.bat run -PhlsRenderApi=OPENGL
```

Accepted values are `SOFTWARE`, `ANGLE`, `DIRECT3D`, and `OPENGL`. Release builds should keep the default unless a renderer-specific validation run is intentional. `OPENGL` is a diagnostic option for supported Windows architectures; on Windows ARM64 prefer `ANGLE` or `DIRECT3D`.
