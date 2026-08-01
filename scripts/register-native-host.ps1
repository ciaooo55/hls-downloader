param(
    [switch]$Unregister,
    [string]$RegistryPrefix = "HKCU:\Software",
    [string]$HostExecutable = ""
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$name = "com.ciaooo55.hls_downloader"
$chrome = Join-Path $RegistryPrefix "Google\Chrome\NativeMessagingHosts\$name"
$edge = Join-Path $RegistryPrefix "Microsoft\Edge\NativeMessagingHosts\$name"
$brave = Join-Path $RegistryPrefix "BraveSoftware\Brave-Browser\NativeMessagingHosts\$name"
$chromium = Join-Path $RegistryPrefix "Chromium\NativeMessagingHosts\$name"
$vivaldi = Join-Path $RegistryPrefix "Vivaldi\NativeMessagingHosts\$name"
$opera = Join-Path $RegistryPrefix "Opera Software\NativeMessagingHosts\$name"
$firefox = Join-Path $RegistryPrefix "Mozilla\NativeMessagingHosts\$name"
if ($Unregister) {
    Remove-Item -LiteralPath $chrome, $edge, $brave, $chromium, $vivaldi, $opera, $firefox -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "Browser integration removed."
    exit 0
}
$manifestDir = Join-Path $root "native-host"
$versionsDir = Join-Path $manifestDir "versions"
$manifestsDir = Join-Path $manifestDir "manifests"

function Get-VersionedNativeHost {
    param([string]$Directory)

    if (-not (Test-Path -LiteralPath $Directory)) { return $null }
    $candidates = @(
        Get-ChildItem -LiteralPath $Directory -Filter "HLSDownloaderNativeHost-*.exe" -File -ErrorAction SilentlyContinue |
        ForEach-Object {
            if ($_.BaseName -match '^HLSDownloaderNativeHost-(?<version>\d+(?:\.\d+){0,3})$') {
                [PSCustomObject]@{ File = $_; Version = [version]$Matches.version }
            }
        } |
        Sort-Object -Property Version -Descending
    )
    if ($candidates.Count) { return $candidates[0].File.FullName }
    return $null
}

function Get-VersionedManifest {
    param(
        [string]$Directory,
        [string]$Name
    )

    if (-not (Test-Path -LiteralPath $Directory)) { return $null }
    $escapedName = [regex]::Escape($Name)
    $candidates = @(
        Get-ChildItem -LiteralPath $Directory -Filter "$Name-*.json" -File -ErrorAction SilentlyContinue |
        ForEach-Object {
            if ($_.BaseName -match "^$escapedName-(?<version>\d+(?:\.\d+){0,3})$") {
                [PSCustomObject]@{ File = $_; Version = [version]$Matches.version }
            }
        } |
        Sort-Object -Property Version -Descending
    )
    if ($candidates.Count) { return $candidates[0].File.FullName }
    return $null
}

if (-not $HostExecutable) {
    # New installers use a versioned path so a browser-owned process never
    # locks the file that has to be replaced by a later update.  The root-path
    # fallback keeps portable and pre-versioned installs working.
    $HostExecutable = Get-VersionedNativeHost -Directory $versionsDir
    if (-not $HostExecutable) {
        $HostExecutable = Join-Path $root "HLSDownloaderNativeHost.exe"
    }
}
$hostExecutable = [IO.Path]::GetFullPath($HostExecutable)
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
if (-not (Test-Path -LiteralPath $hostExecutable)) {
    throw "Native Messaging host executable not found: $hostExecutable"
}
$chromeManifest = Get-VersionedManifest -Directory $manifestsDir -Name "chrome"
if (-not $chromeManifest) { $chromeManifest = Join-Path $manifestDir "chrome.json" }
$firefoxManifest = Get-VersionedManifest -Directory $manifestsDir -Name "firefox"
if (-not $firefoxManifest) { $firefoxManifest = Join-Path $manifestDir "firefox.json" }
foreach ($entry in @(
    @($chrome, $chromeManifest),
    @($edge, $chromeManifest),
    @($brave, $chromeManifest),
    @($chromium, $chromeManifest),
    @($vivaldi, $chromeManifest),
    @($opera, $chromeManifest),
    @($firefox, $firefoxManifest)
)) {
    $manifestPath = $entry[1]
    if (-not (Test-Path -LiteralPath $manifestPath)) {
        throw "Native Messaging manifest not found: $manifestPath"
    }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $manifest.path = $hostExecutable
    [System.IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 8), $utf8NoBom)
    New-Item -Path $entry[0] -Force | Out-Null
    Set-Item -Path $entry[0] -Value $manifestPath
}

# Remove only hosts which are neither the current registration target nor a
# live Native Messaging process.  A failed cleanup is intentionally harmless:
# it must never turn a successful update into a failed one.
$runningHosts = @()
if (Test-Path -LiteralPath $versionsDir) {
    $runningHosts = @(
        Get-Process -Name "HLSDownloaderNativeHost*" -ErrorAction SilentlyContinue |
        ForEach-Object { $_.Path } |
        Where-Object { $_ } |
        ForEach-Object { [IO.Path]::GetFullPath($_) }
    )
    Get-ChildItem -LiteralPath $versionsDir -Filter "HLSDownloaderNativeHost-*.exe" -File -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -ne $hostExecutable -and $_.FullName -notin $runningHosts } |
        ForEach-Object {
            Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue
        }
}
$legacyHost = Join-Path $root "HLSDownloaderNativeHost.exe"
if ((Test-Path -LiteralPath $legacyHost) -and $legacyHost -ne $hostExecutable -and $legacyHost -notin $runningHosts) {
    Remove-Item -LiteralPath $legacyHost -Force -ErrorAction SilentlyContinue
}
# Retire old manifests only after the registry points at the new paths.  An
# in-flight browser launch can still use the old file; a failed removal is safe.
if (Test-Path -LiteralPath $manifestsDir) {
    Get-ChildItem -LiteralPath $manifestsDir -Filter "*.json" -File -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -ne $chromeManifest -and $_.FullName -ne $firefoxManifest } |
        ForEach-Object {
            Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue
        }
}
Write-Host "Chrome, Edge, Brave, Chromium, Vivaldi, Opera and Firefox Native Messaging hosts registered."
