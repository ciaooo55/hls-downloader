[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
$env:CARGO_HOME = 'E:\HLSDownloaderBuildCache\cargo'
$env:CARGO_TARGET_DIR = 'D:\HLSDownloaderBuildCache\cargo-target'
$env:GRADLE_USER_HOME = 'E:\HLSDownloaderBuildCache\gradle'
$env:JAVA_HOME = 'E:\HLSDownloaderBuildCache\jdk-21'

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
