[CmdletBinding()]
param(
    [string]$EngineExecutable = '',
    [switch]$Unregister
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

if ([String]::IsNullOrWhiteSpace($EngineExecutable)) {
    $cacheRoot = if ($env:HLS_V7_BUILD_CACHE) { [IO.Path]::GetFullPath($env:HLS_V7_BUILD_CACHE) } else { Join-Path $root '.tool-cache\build-cache' }
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $cacheRoot 'cargo-target' }
    foreach ($candidate in @(
        (Join-Path $target 'release\hls-downloader-engine.exe'),
        (Join-Path $target 'debug\hls-downloader-engine.exe'),
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
$process = Start-Process -FilePath ([IO.Path]::GetFullPath($EngineExecutable)) -ArgumentList $argument -NoNewWindow -Wait -PassThru
if ($process.ExitCode -ne 0) {
    throw "v7 Native Host command failed with exit $($process.ExitCode)"
}
