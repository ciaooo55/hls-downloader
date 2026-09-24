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

# 说明：本固定方式已被证明会腐烂，务必读这段再改。
# 上游 BtbN 只保留最近约 5 个 autobuild 发布，所以「固定到某个 autobuild 日期」
# 必然随时间失效：原先固定的 autobuild-2026-09-06-13-06 已被上游删除，资产返回
# 404，导致 bootstrap、本地候选打包与 package-v7-candidate CI 同时失败。
# 2026-09-20 重新固定到当前仍存在的 autobuild-2026-09-19-13-11 静态 win64-gpl 构建
# （FFmpeg 9.0.1 → 9.0.2，同为不带独立 DLL 集的静态构建），并记录实测 SHA-256。
# 供应链属性不变：仍为「固定版本 + 固定摘要 + 下载后校验」。
# 若日后再遇 404，按同样流程重新固定并在此注明原因与日期。
$ffmpegRelease = 'autobuild-2026-09-19-13-11'
$ffmpegArchiveName = 'ffmpeg-n9.0.2-win64-gpl-9.0.zip'
$ffmpegArchiveUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/$ffmpegRelease/$ffmpegArchiveName"
$ffmpegSha256 = '44083538105b4e64d439f9e67bd875bd264b4271239c808b2acea09773ad1aa3'
$toolRoot = Join-Path $cacheRoot 'ffmpeg-n9.0.2-win64-gpl-9.0'
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
