[CmdletBinding()]
param([string]$JdkRoot='',[string]$GradleHome='')
$ErrorActionPreference='Stop'
$repo=(Resolve-Path "$PSScriptRoot\..").Path

# Toolchain downloads default inside the repository; HLS_V7_BUILD_CACHE relocates bootstrap/build/cleanup together.
if ($env:HLS_V7_BUILD_CACHE -and -not [IO.Path]::IsPathRooted($env:HLS_V7_BUILD_CACHE)) { throw 'HLS_V7_BUILD_CACHE must be an absolute path.' }
$cacheRoot = if ($env:HLS_V7_BUILD_CACHE) { [IO.Path]::GetFullPath($env:HLS_V7_BUILD_CACHE) } else { Join-Path $repo '.tool-cache\build-cache' }
$customJdkRoot = -not [string]::IsNullOrWhiteSpace($JdkRoot)
if(-not $JdkRoot){ $JdkRoot = Join-Path $cacheRoot 'jdk-21' }
if(-not $GradleHome){ $GradleHome = Join-Path $cacheRoot 'gradle' }

# Keep local/release builds reproducible instead of following Adoptium's moving "latest" endpoint.
$jdkVersion = '21.0.12.1+1'
$jdkRuntimeVersion = '21.0.12.1'
$jdkArchiveName = 'OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip'
$jdkArchiveUrl = 'https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jdk_x64_windows_hotspot_21.0.12.1_1.zip'
$jdkSha256 = 'f9d6e191ab098c0d416e7d588a24420a8621cd2f4720dab2459b8b7b2d2d8b4e'

function Test-ExpectedJdk([string]$Root) {
    $java = Join-Path $Root 'bin\java.exe'
    if (-not (Test-Path -LiteralPath $java)) { return $false }
    $versionOutput = (& $java -version 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) { return $false }
    return $versionOutput -match ('version\s+"' + [regex]::Escape($jdkRuntimeVersion) + '"')
}

if (Test-ExpectedJdk $JdkRoot) {
    Write-Output "JDK already ready: Temurin $jdkVersion at $JdkRoot"
    exit 0
}

if (Test-Path -LiteralPath $JdkRoot) {
    if ($customJdkRoot) {
        throw "Custom JdkRoot exists but is not the required JDK ${jdkVersion}: $JdkRoot"
    }
    Remove-Item -LiteralPath $JdkRoot -Recurse -Force
}

$downloadsRoot = Join-Path $cacheRoot 'downloads'
New-Item -ItemType Directory -Force -Path $GradleHome,$downloadsRoot,(Split-Path $JdkRoot -Parent) | Out-Null
$zip = Join-Path $downloadsRoot $jdkArchiveName
$unpack = Join-Path $downloadsRoot 'temurin-jdk-21.0.12.1_1-windows-x64'

if (Test-Path -LiteralPath $zip) {
    $cachedHash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($cachedHash -ne $jdkSha256) {
        Remove-Item -LiteralPath $zip -Force
    }
}

if (-not (Test-Path -LiteralPath $zip)) {
    Write-Output "Downloading Temurin JDK $jdkVersion..."
    Invoke-WebRequest -UseBasicParsing -Uri $jdkArchiveUrl -OutFile $zip
}

$actualHash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualHash -ne $jdkSha256) {
    Remove-Item -LiteralPath $zip -Force -ErrorAction SilentlyContinue
    throw "Temurin JDK archive SHA-256 mismatch. Expected $jdkSha256, got $actualHash."
}

Remove-Item -LiteralPath $unpack -Recurse -Force -ErrorAction SilentlyContinue
try {
    Expand-Archive -LiteralPath $zip -DestinationPath $unpack -Force
    $sources = @(Get-ChildItem -LiteralPath $unpack -Directory)
    if ($sources.Count -ne 1) { throw "Expected exactly one JDK root in $jdkArchiveName; found $($sources.Count)." }
    $source = $sources[0]
    if (-not (Test-ExpectedJdk $source.FullName)) { throw "Downloaded archive does not contain the required JDK runtime $jdkRuntimeVersion." }
    Move-Item -LiteralPath $source.FullName -Destination $JdkRoot
} finally {
    Remove-Item -LiteralPath $unpack -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not (Test-ExpectedJdk $JdkRoot)) { throw "JDK installation verification failed: $JdkRoot" }
Write-Output "JDK installed and verified: Temurin $jdkVersion at $JdkRoot"
