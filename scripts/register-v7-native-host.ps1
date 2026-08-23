[CmdletBinding()]
param(
    [string]$HostExecutable = "",
    [string]$RegistryPrefix = "HKCU:\Software",
    [switch]$Unregister
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot\..").Path
$hostName = 'com.ciaooo55.hls_downloader'
$legacyHostName = 'com.ciaooo55.hls_downloader.v6'
$runtimeDir = Join-Path $env:LOCALAPPDATA 'HLSDownloader\v7-native-host'

function Get-RegistryPaths {
    param([string]$Name)
    @(
        (Join-Path $RegistryPrefix "Google\Chrome\NativeMessagingHosts\$Name"),
        (Join-Path $RegistryPrefix "Microsoft\Edge\NativeMessagingHosts\$Name"),
        (Join-Path $RegistryPrefix "BraveSoftware\Brave-Browser\NativeMessagingHosts\$Name"),
        (Join-Path $RegistryPrefix "Chromium\NativeMessagingHosts\$Name"),
        (Join-Path $RegistryPrefix "Vivaldi\NativeMessagingHosts\$Name"),
        (Join-Path $RegistryPrefix "Opera Software\NativeMessagingHosts\$Name"),
        (Join-Path $RegistryPrefix "Mozilla\NativeMessagingHosts\$Name")
    )
}

$registryPaths = Get-RegistryPaths $hostName
$legacyRegistryPaths = Get-RegistryPaths $legacyHostName
if ($Unregister) {
    foreach ($path in $registryPaths) {
        Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $runtimeDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host 'v7 Native Host registration removed.'
    exit 0
}

foreach ($path in $legacyRegistryPaths) {
    Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue
}

if (-not $HostExecutable) {
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'D:\HLSDownloaderBuildCache\cargo-target' }
    foreach ($candidate in @(
        (Join-Path $target 'release\HLSDownloaderNativeHost.exe'),
        (Join-Path $target 'debug\HLSDownloaderNativeHost.exe'),
        (Join-Path $root 'desktop_ui\resources\common\HLSDownloaderNativeHost.exe'),
        (Join-Path $root 'app\resources\HLSDownloaderNativeHost.exe'),
        (Join-Path $env:USERPROFILE 'Desktop\HLSDownloader-7.0.0-NativeHost.exe')
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $HostExecutable = $candidate
            break
        }
    }
}
if (-not $HostExecutable -or -not (Test-Path -LiteralPath $HostExecutable -PathType Leaf)) {
    throw 'v7 Native Host executable not found. Run scripts\build-v7.ps1 -Task test or package first.'
}

New-Item -ItemType Directory -Force -Path $runtimeDir | Out-Null
$hostExecutable = [IO.Path]::GetFullPath($HostExecutable)
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$sources = @{
    'chrome.json' = if (Test-Path (Join-Path $root 'native-host\chrome.json')) { Join-Path $root 'native-host\chrome.json' } else { Join-Path $root 'extension\native-host\chrome.json' }
    'firefox.json' = if (Test-Path (Join-Path $root 'native-host\firefox.json')) { Join-Path $root 'native-host\firefox.json' } else { Join-Path $root 'extension\native-host\firefox.json' }
}
foreach ($entry in $sources.GetEnumerator()) {
    $manifest = Get-Content -LiteralPath $entry.Value -Raw -Encoding UTF8 | ConvertFrom-Json
    $manifest.name = $hostName
    $manifest.description = 'HLS Downloader 7.0.0 Native Messaging Host'
    $manifest.path = $hostExecutable
    $destination = Join-Path $runtimeDir ("v7-" + $entry.Key)
    [IO.File]::WriteAllText($destination, ($manifest | ConvertTo-Json -Depth 8), $utf8NoBom)
}

$chromeManifest = Join-Path $runtimeDir 'v7-chrome.json'
$firefoxManifest = Join-Path $runtimeDir 'v7-firefox.json'
foreach ($path in $registryPaths[0..5]) {
    New-Item -Path $path -Force | Out-Null
    Set-Item -LiteralPath $path -Value $chromeManifest
}
New-Item -Path $registryPaths[6] -Force | Out-Null
Set-Item -LiteralPath $registryPaths[6] -Value $firefoxManifest
Write-Host "v7 Native Host registered: $hostExecutable"
