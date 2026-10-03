# HLS Downloader Desktop 7.0.2

Compose Desktop workbench for HLS Downloader. This module is the 7.x desktop interface; `HLSDownloaderEngine.exe` owns all download state and SQLite through the existing framed IPC protocol.

```powershell
# From the repository root; downloads and build outputs stay in .tool-cache/build-cache.
.\scripts\bootstrap-v7-toolchain.ps1
.\scripts\run-v7-local.ps1
```

The packaged product defaults to Skiko software rendering for broad Windows compatibility. For diagnostics or performance comparison, opt into a different backend without editing source:

```powershell
# From desktop_ui, reuse the repository-local toolchain and dependency cache.
$cacheRoot = Join-Path (Resolve-Path ..).Path '.tool-cache\build-cache'
$env:GRADLE_USER_HOME = Join-Path $cacheRoot 'gradle'
$env:JAVA_HOME = Join-Path $cacheRoot 'jdk-21'
$env:HLS_UI_RENDER_API = 'ANGLE'
.\gradlew.bat run

$env:HLS_UI_RENDER_API = 'DIRECT3D'
.\gradlew.bat run

# Equivalent Gradle property form
.\gradlew.bat run -PhlsRenderApi=OPENGL
```

Accepted values are `SOFTWARE`, `ANGLE`, `DIRECT3D`, and `OPENGL`. Release builds should keep the default unless a renderer-specific validation run is intentional. `OPENGL` is a diagnostic option for supported Windows architectures; on Windows ARM64 prefer `ANGLE` or `DIRECT3D`.
