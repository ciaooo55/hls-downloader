[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
# Build caches default inside the repository; HLS_V7_BUILD_CACHE relocates them.
$cacheRoot = if ($env:HLS_V7_BUILD_CACHE) { $env:HLS_V7_BUILD_CACHE } else { Join-Path $repo '.tool-cache\build-cache' }
$env:CARGO_HOME = Join-Path $cacheRoot 'cargo'
$env:CARGO_TARGET_DIR = Join-Path $cacheRoot 'cargo-target'
$env:GRADLE_USER_HOME = Join-Path $cacheRoot 'gradle'
$env:HLS_COMPOSE_BUILD_DIR = Join-Path $cacheRoot 'compose-build'
$jdkRoot = $env:HLS_V7_JAVA_HOME
if(-not $jdkRoot -and (Test-Path (Join-Path $cacheRoot 'jdk-21\bin\java.exe'))){ $jdkRoot = Join-Path $cacheRoot 'jdk-21' }
# Legacy read-only tool location from earlier installs; tools are not project content.
if(-not $jdkRoot -and (Test-Path 'E:\HLSDownloaderBuildCache\jdk-21\bin\java.exe')){ $jdkRoot = 'E:\HLSDownloaderBuildCache\jdk-21' }
if(-not $jdkRoot){ throw 'JDK 21 was not found. Set HLS_V7_JAVA_HOME or run scripts\bootstrap-v7-toolchain.ps1.' }
$env:JAVA_HOME = $jdkRoot

if (-not (Test-Path -LiteralPath (Join-Path $env:JAVA_HOME 'bin\java.exe'))) {
    throw "JDK 21 is missing at $env:JAVA_HOME"
}

Push-Location $repo
try {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task run
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Pop-Location
}
