# Historical Compose Desktop Migration Reference

This Kotlin / Compose Desktop module is retained as a historical migration reference. It is not the current desktop interface: the stable product uses the Rust / Slint workbench in [`native_ui`](../native_ui/). The independent Rust Core owns download state and SQLite through the framed IPC protocol.

For current source-build and test instructions, see the root [English README](../README_EN.md#-build-from-source) or [简体中文 README](../README.md).

This historical Compose module defaults to Skiko software rendering. For diagnostics or performance comparison within this module, opt into a different backend without editing source:

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

Accepted values are `SOFTWARE`, `ANGLE`, `DIRECT3D`, and `OPENGL`. These options apply only to the historical Compose module, not the current native workbench. The packaged product supports Windows x64 only; these diagnostic backend options do not imply ARM64 support.
