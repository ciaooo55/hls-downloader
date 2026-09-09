[CmdletBinding()]
param(
    [string]$EngineExecutable = '',
    # Retained for compatibility with older local-install invocations. The
    # registration implementation itself is owned by the sibling Engine.
    [string]$HostExecutable = '',
    [switch]$Unregister
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

if ([String]::IsNullOrWhiteSpace($EngineExecutable) -and -not [String]::IsNullOrWhiteSpace($HostExecutable)) {
    $EngineExecutable = Join-Path ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($HostExecutable))) 'HLSDownloaderEngine.exe'
}
if ([String]::IsNullOrWhiteSpace($EngineExecutable)) {
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root '.tool-cache\build-cache\cargo-target' }
    foreach ($candidate in @(
        (Join-Path $target 'release\hls-downloader-engine.exe'),
        (Join-Path $target 'debug\hls-downloader-engine.exe'),
        (Join-Path $root 'desktop_ui\resources\common\HLSDownloaderEngine.exe'),
        (Join-Path $root 'app\resources\HLSDownloaderEngine.exe')
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $EngineExecutable = $candidate
            break
        }
    }
}
if ([String]::IsNullOrWhiteSpace($EngineExecutable) -or -not (Test-Path -LiteralPath $EngineExecutable -PathType Leaf)) {
    throw 'v7 Engine executable not found. Build the project or pass -EngineExecutable.'
}

$argument = if ($Unregister) { '--unregister-native-host' } else { '--register-native-host' }
& ([IO.Path]::GetFullPath($EngineExecutable)) $argument
if ($LASTEXITCODE -ne 0) {
    throw "v7 Native Host command failed with exit $LASTEXITCODE"
}
