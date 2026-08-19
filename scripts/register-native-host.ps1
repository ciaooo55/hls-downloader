param(
    [switch]$Unregister,
    [string]$RegistryPrefix = "HKCU:\Software",
    [string]$HostExecutable = "",
    [switch]$V6,
    [switch]$Cutover
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$name = if ($V6 -and -not $Cutover) {
    "com.ciaooo55.hls_downloader.v6"
} else {
    "com.ciaooo55.hls_downloader"
}
$v6Name = "com.ciaooo55.hls_downloader.v6"
function Get-NativeMessagingRegistryPaths {
    param([string]$HostName)
    return @{
        Chrome = Join-Path $RegistryPrefix "Google\Chrome\NativeMessagingHosts\$HostName"
        Edge = Join-Path $RegistryPrefix "Microsoft\Edge\NativeMessagingHosts\$HostName"
        Brave = Join-Path $RegistryPrefix "BraveSoftware\Brave-Browser\NativeMessagingHosts\$HostName"
        Chromium = Join-Path $RegistryPrefix "Chromium\NativeMessagingHosts\$HostName"
        Vivaldi = Join-Path $RegistryPrefix "Vivaldi\NativeMessagingHosts\$HostName"
        Opera = Join-Path $RegistryPrefix "Opera Software\NativeMessagingHosts\$HostName"
        Firefox = Join-Path $RegistryPrefix "Mozilla\NativeMessagingHosts\$HostName"
    }
}

$chrome = (Get-NativeMessagingRegistryPaths -HostName $name).Chrome
$edge = (Get-NativeMessagingRegistryPaths -HostName $name).Edge
$brave = (Get-NativeMessagingRegistryPaths -HostName $name).Brave
$chromium = (Get-NativeMessagingRegistryPaths -HostName $name).Chromium
$vivaldi = (Get-NativeMessagingRegistryPaths -HostName $name).Vivaldi
$opera = (Get-NativeMessagingRegistryPaths -HostName $name).Opera
$firefox = (Get-NativeMessagingRegistryPaths -HostName $name).Firefox
if ($Unregister) {
    $paths = @()
    foreach ($hostName in @($name, $v6Name, "com.ciaooo55.hls_downloader")) {
        $paths += (Get-NativeMessagingRegistryPaths -HostName $hostName).Values
    }
    Remove-Item -LiteralPath ($paths | Select-Object -Unique) -Recurse -Force -ErrorAction SilentlyContinue
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
}
if (-not $HostExecutable -or -not (Test-Path -LiteralPath $HostExecutable)) {
    foreach ($candidate in @(
        (Join-Path $root "HLSDownloaderNativeHost.exe"),
        (Join-Path $root "HLSDownloader.exe"),
        (Join-Path $root "build\v6\stage\HLSDownloaderNativeHost.exe"),
        (Join-Path $root "native_ui\target\release\HLSDownloaderNativeHost.exe"),
        (Join-Path $root "native_ui\target\release\HLSDownloader.exe")
    )) {
        if (Test-Path -LiteralPath $candidate) {
            $HostExecutable = $candidate
            break
        }
    }
}
if (-not $HostExecutable) {
    $HostExecutable = Join-Path $root "HLSDownloaderNativeHost.exe"
}
$hostExecutable = [IO.Path]::GetFullPath($HostExecutable)
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
if (-not (Test-Path -LiteralPath $hostExecutable)) {
    throw "Native Messaging host executable not found: $hostExecutable"
}
$isSourceTree = Test-Path -LiteralPath (Join-Path $root "extension\native-host")
function Get-RuntimeManifestDir {
    if ($isSourceTree) {
        $runtimeDir = Join-Path $env:LOCALAPPDATA "HLSDownloader\native-host"
        New-Item -ItemType Directory -Force -Path $runtimeDir | Out-Null
        return $runtimeDir
    }
    New-Item -ItemType Directory -Force -Path $manifestDir | Out-Null
    return $manifestDir
}
function Install-NativeHostManifest {
    param(
        [string]$SourcePath,
        [string]$FileName,
        [string]$HostName
    )
    if (-not $SourcePath -or -not (Test-Path -LiteralPath $SourcePath -PathType Leaf)) {
        throw "Native Messaging manifest not found: $SourcePath"
    }
    $dest = Join-Path (Get-RuntimeManifestDir) $FileName
    $srcFull = [IO.Path]::GetFullPath($SourcePath)
    $destFull = [IO.Path]::GetFullPath($dest)
    if ($srcFull -ne $destFull) {
        Copy-Item -LiteralPath $srcFull -Destination $destFull -Force
    }
    $manifest = Get-Content -LiteralPath $destFull -Raw -Encoding UTF8 | ConvertFrom-Json
    $manifest.name = $HostName
    $manifest.path = $hostExecutable
    [System.IO.File]::WriteAllText($destFull, ($manifest | ConvertTo-Json -Depth 8), $utf8NoBom)
    return $destFull
}
function Register-NativeHostManifest {
    param(
        [string]$ManifestPath,
        [object]$RegistryPaths
    )
    foreach ($regPath in $RegistryPaths) {
        New-Item -Path $regPath -Force | Out-Null
        Set-Item -Path $regPath -Value $ManifestPath
    }
}
$selectedHostVersion = ""
if ([IO.Path]::GetFileNameWithoutExtension($hostExecutable) -match '^HLSDownloaderNativeHost-(?<version>\d+(?:\.\d+){0,3})$') {
    $selectedHostVersion = $Matches.version
}
$chromeManifest = if ($selectedHostVersion) {
    Join-Path $manifestsDir "chrome-$selectedHostVersion.json"
} else { $null }
if (-not $chromeManifest -or -not (Test-Path -LiteralPath $chromeManifest -PathType Leaf)) {
    $chromeManifest = Get-VersionedManifest -Directory $manifestsDir -Name "chrome"
}
if (-not $chromeManifest) { $chromeManifest = Join-Path $manifestDir "chrome.json" }
$firefoxManifest = if ($selectedHostVersion) {
    Join-Path $manifestsDir "firefox-$selectedHostVersion.json"
} else { $null }
if (-not $firefoxManifest -or -not (Test-Path -LiteralPath $firefoxManifest -PathType Leaf)) {
    $firefoxManifest = Get-VersionedManifest -Directory $manifestsDir -Name "firefox"
}
if (-not $firefoxManifest) { $firefoxManifest = Join-Path $manifestDir "firefox.json" }
$repoChrome = Join-Path $root "extension\native-host\chrome.json"
$repoFirefox = Join-Path $root "extension\native-host\firefox.json"
$repoV6Chrome = Join-Path $root "extension\native-host\v6-chrome.json"
$packagedV6Chrome = Join-Path $manifestDir "v6-chrome.json"
$repoV6Firefox = Join-Path $root "extension\native-host\v6-firefox.json"
$packagedV6Firefox = Join-Path $manifestDir "v6-firefox.json"
function First-ExistingManifest {
    param([string[]]$Candidates)
    foreach ($candidate in $Candidates) {
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            return $candidate
        }
    }
    return $null
}
$v6ChromeSource = First-ExistingManifest @($packagedV6Chrome, $repoV6Chrome)
$v6FirefoxSource = First-ExistingManifest @($packagedV6Firefox, $repoV6Firefox)
if ($V6 -and -not $Cutover) {
    if ($v6ChromeSource) { $chromeManifest = $v6ChromeSource }
    if ($v6FirefoxSource) { $firefoxManifest = $v6FirefoxSource }
} elseif ($Cutover) {
    $chromeManifest = First-ExistingManifest @($chromeManifest, $repoChrome, $v6ChromeSource)
    $firefoxManifest = First-ExistingManifest @($firefoxManifest, $repoFirefox, $v6FirefoxSource)
} else {
    $chromeManifest = First-ExistingManifest @($chromeManifest, $repoChrome)
    $firefoxManifest = First-ExistingManifest @($firefoxManifest, $repoFirefox)
}
$chromeManifestPath = Install-NativeHostManifest -SourcePath $chromeManifest -FileName $(if ($V6 -and -not $Cutover) { "v6-chrome.json" } else { "chrome.json" }) -HostName $name
$firefoxManifestPath = Install-NativeHostManifest -SourcePath $firefoxManifest -FileName $(if ($V6 -and -not $Cutover) { "v6-firefox.json" } else { "firefox.json" }) -HostName $name
Register-NativeHostManifest -ManifestPath $chromeManifestPath -RegistryPaths @($chrome, $edge, $brave, $chromium, $vivaldi, $opera)
Register-NativeHostManifest -ManifestPath $firefoxManifestPath -RegistryPaths @($firefox)

if ($Cutover -and $v6ChromeSource -and $v6FirefoxSource) {
    $v6Paths = Get-NativeMessagingRegistryPaths -HostName $v6Name
    $v6ChromeManifestPath = Install-NativeHostManifest -SourcePath $v6ChromeSource -FileName "v6-chrome.json" -HostName $v6Name
    $v6FirefoxManifestPath = Install-NativeHostManifest -SourcePath $v6FirefoxSource -FileName "v6-firefox.json" -HostName $v6Name
    Register-NativeHostManifest -ManifestPath $v6ChromeManifestPath -RegistryPaths @($v6Paths.Chrome, $v6Paths.Edge, $v6Paths.Brave, $v6Paths.Chromium, $v6Paths.Vivaldi, $v6Paths.Opera)
    Register-NativeHostManifest -ManifestPath $v6FirefoxManifestPath -RegistryPaths @($v6Paths.Firefox)
}

# Remove only hosts which are neither the current registration target nor a
# live Native Messaging process.  A failed cleanup is intentionally harmless:
# it must never turn a successful update into a failed one.
$runningHosts = @()
if (-not $isSourceTree -and (Test-Path -LiteralPath $versionsDir)) {
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
if (-not $isSourceTree -and (Test-Path -LiteralPath $legacyHost) -and $legacyHost -ne $hostExecutable -and $legacyHost -notin $runningHosts) {
    Remove-Item -LiteralPath $legacyHost -Force -ErrorAction SilentlyContinue
}
# Retire old manifests only after the registry points at the new paths.  An
# in-flight browser launch can still use the old file; a failed removal is safe.
if (Test-Path -LiteralPath $manifestsDir) {
    Get-ChildItem -LiteralPath $manifestsDir -Filter "*.json" -File -ErrorAction SilentlyContinue |
        Where-Object {
            $_.FullName -ne $chromeManifest -and
            $_.FullName -ne $firefoxManifest -and
            $_.Name -notlike "v6-*"
        } |
        ForEach-Object {
            Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue
        }
}
Write-Host "Chrome, Edge, Brave, Chromium, Vivaldi, Opera and Firefox Native Messaging hosts registered ($name)."
if ($Cutover) {
    Write-Host "Cutover: 5.x host name now points at the v6 Native Messaging executable."
}
