[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path

if ($env:HLS_V7_BUILD_CACHE -and -not [IO.Path]::IsPathRooted($env:HLS_V7_BUILD_CACHE)) {
    throw 'HLS_V7_BUILD_CACHE must be an absolute path.'
}
$cacheRoot = if ($env:HLS_V7_BUILD_CACHE) {
    [IO.Path]::GetFullPath($env:HLS_V7_BUILD_CACHE)
} else {
    Join-Path $repo '.tool-cache\build-cache'
}

# A dated BtbN release plus an asset digest keeps release packaging independent
# of moving FFmpeg aliases. Use the static Windows build because build-v7.ps1
# intentionally bundles the three executables without a separate DLL set.
$ffmpegRelease = 'autobuild-2026-09-06-13-06'
$ffmpegArchiveName = 'ffmpeg-n9.0.1-26-g5c8e7e2433-win64-gpl-9.0.zip'
$ffmpegArchiveUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$ffmpegRelease/$ffmpegArchiveName"
$ffmpegSha256 = 'dd232ccf8661f837a1faa5f534a1a0bdbdb25c42afe79391e8345154df78f791'
$toolRoot = Join-Path $cacheRoot 'ffmpeg-n9.0.1-26-g5c8e7e2433-win64-gpl-9.0'
$downloadsRoot = Join-Path $cacheRoot 'downloads'
$archivePath = Join-Path $downloadsRoot $ffmpegArchiveName

function Get-MediaBin([string]$Root) {
    if (-not (Test-Path -LiteralPath $Root -PathType Container)) { return $null }
    $ffmpeg = Get-ChildItem -LiteralPath $Root -Filter 'ffmpeg.exe' -File -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $ffmpeg) { return $null }
    $bin = $ffmpeg.Directory.FullName
    foreach ($name in @('ffmpeg.exe', 'ffprobe.exe', 'ffplay.exe')) {
        if (-not (Test-Path -LiteralPath (Join-Path $bin $name) -PathType Leaf)) { return $null }
    }
    return $bin
}

$ready = Get-MediaBin $toolRoot
if ($ready) {
    Write-Output $ready
    exit 0
}

if (Test-Path -LiteralPath $toolRoot) {
    Remove-Item -LiteralPath $toolRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $downloadsRoot | Out-Null

if (Test-Path -LiteralPath $archivePath) {
    $cachedHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($cachedHash -ne $ffmpegSha256) {
        Remove-Item -LiteralPath $archivePath -Force
    }
}

if (-not (Test-Path -LiteralPath $archivePath)) {
    Write-Host "Downloading pinned FFmpeg asset $ffmpegArchiveName..."
    Invoke-WebRequest -UseBasicParsing -Uri $ffmpegArchiveUrl -OutFile $archivePath
}

$actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualHash -ne $ffmpegSha256) {
    Remove-Item -LiteralPath $archivePath -Force -ErrorAction SilentlyContinue
    throw "FFmpeg archive SHA-256 mismatch. Expected $ffmpegSha256, got $actualHash."
}

$tempRoot = Join-Path $downloadsRoot ('.ffmpeg-extract-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
try {
    Expand-Archive -LiteralPath $archivePath -DestinationPath $tempRoot -Force
    $bin = Get-MediaBin $tempRoot
    if (-not $bin) {
        throw 'Verified FFmpeg archive does not contain ffmpeg.exe, ffprobe.exe and ffplay.exe in one directory.'
    }
    $sourceRoot = Split-Path $bin -Parent
    Move-Item -LiteralPath $sourceRoot -Destination $toolRoot
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}

$ready = Get-MediaBin $toolRoot
if (-not $ready) { throw "FFmpeg installation verification failed: $toolRoot" }
Write-Output $ready
