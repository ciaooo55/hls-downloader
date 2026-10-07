[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'V7HashFunctions.ps1')
$repo = (Resolve-Path "$PSScriptRoot\..").Path

if ($env:HLS_V7_BUILD_CACHE -and -not [IO.Path]::IsPathRooted($env:HLS_V7_BUILD_CACHE)) {
    throw 'HLS_V7_BUILD_CACHE must be an absolute path.'
}
$cacheRoot = if ($env:HLS_V7_BUILD_CACHE) {
    [IO.Path]::GetFullPath($env:HLS_V7_BUILD_CACHE)
} else {
    Join-Path $repo '.tool-cache\build-cache'
}

# BtbN 每月最后一次构建保留两年，日常构建只保留 14 天。
# 使用月末固定构建及摘要，避免清洁 CI 随日常资产删除而立即失效。
$ffmpegRelease = 'autobuild-2026-09-30-13-08'
$ffmpegArchiveName = 'ffmpeg-n9.0.2-17-g2a571b6068-win64-gpl-9.0.zip'
$ffmpegArchiveUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$ffmpegRelease/$ffmpegArchiveName"
$ffmpegSha256 = 'a0e45723c72141975f51d8666302e614711745f3102b704ca3f82c897a58d278'
$toolRoot = Join-Path $cacheRoot 'ffmpeg-n9.0.2-17-g2a571b6068-win64-gpl-9.0'
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
    Invoke-WebRequest -UseBasicParsing -Uri $ffmpegArchiveUrl -OutFile $archivePath -TimeoutSec 180
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
