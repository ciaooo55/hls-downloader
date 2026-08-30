[CmdletBinding()]
param(
    [string]$SourceDir = 'D:\HLSDownloaderBuildCache\compose-build\compose\binaries\main\app\HLSDownloader',
    [string]$TargetDir = 'E:\h',
    [string]$ExtensionOutput = '',
    [string]$UpgradeNote = ''
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$installRoot = [IO.Path]::GetFullPath('E:\h').TrimEnd('\', '/')
$source = [IO.Path]::GetFullPath($SourceDir).TrimEnd('\', '/')
$target = [IO.Path]::GetFullPath($TargetDir).TrimEnd('\', '/')
if (-not [String]::Equals($target, $installRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Local install target must be exactly ${installRoot}: $target"
}
foreach ($name in @(
    'HLSDownloader.exe',
    'app',
    'runtime',
    'app\resources\HLSDownloaderEngine.exe',
    'app\resources\HLSDownloaderNativeHost.exe',
    'app\resources\HLSDownloaderUpdater.exe',
    'app\resources\HLSDownloaderPresenter.exe',
    'app\resources\ffmpeg.exe',
    'app\resources\ffprobe.exe',
    'app\resources\libmpv-2.dll',
    'app\resources\BUILD-PROVENANCE.json',
    'app\resources\FEATURE-PARITY.json'
)) {
    if (-not (Test-Path -LiteralPath (Join-Path $source $name))) {
        throw "v7 local image is incomplete; missing ${name}: $source"
    }
}
$provenancePath = Join-Path $source 'app\resources\BUILD-PROVENANCE.json'
$featureParityPath = Join-Path $source 'app\resources\FEATURE-PARITY.json'
$provenance = Get-Content -LiteralPath $provenancePath -Raw -Encoding UTF8 | ConvertFrom-Json
if ($provenance.product_version -ne '7.0.0') {
    throw "v7 local image provenance product_version is not 7.0.0: $($provenance.product_version)"
}
if (@('candidate', 'formal') -notcontains [string]$provenance.package_tier) {
    throw "v7 local image provenance package_tier is invalid: $($provenance.package_tier)"
}
$featureHash = (Get-FileHash -LiteralPath $featureParityPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ([string]$provenance.feature_parity_sha256 -ne $featureHash) {
    throw "v7 local image feature parity hash does not match provenance: $($provenance.feature_parity_sha256) != $featureHash"
}

$extensionRoot = if ([String]::IsNullOrWhiteSpace($ExtensionOutput)) { Join-Path $repo 'extension\.output' } else { [IO.Path]::GetFullPath($ExtensionOutput) }
$packagedExtensionRoot = Join-Path $source 'app\resources\extensions'
$packagedExtensionsReady = @(
    (Join-Path $packagedExtensionRoot 'HLSDownloader-7.0.0-Chromium.zip'),
    (Join-Path $packagedExtensionRoot 'HLSDownloader-7.0.0-Firefox.zip')
) | ForEach-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Where-Object { -not $_ } | Measure-Object | Select-Object -ExpandProperty Count
$note = if ([String]::IsNullOrWhiteSpace($UpgradeNote)) {
    Join-Path $repo 'docs\v7-desktop-upgrade-note.md'
} else {
    [IO.Path]::GetFullPath($UpgradeNote)
}
if ($packagedExtensionsReady -gt 0) {
    foreach ($browser in @('chrome-mv3', 'firefox-mv3')) {
        if (-not (Test-Path -LiteralPath (Join-Path $extensionRoot $browser) -PathType Container)) {
            throw "Production browser extension is missing: $browser"
        }
        $manifest = Get-Content -LiteralPath (Join-Path (Join-Path $extensionRoot $browser) 'manifest.json') -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($manifest.version -ne '7.0.0') { throw "Built $browser extension version is not 7.0.0: $($manifest.version)" }
    }
}
if (-not (Test-Path -LiteralPath $note -PathType Leaf)) {
    throw "Upgrade note is missing: $note"
}

$stage = "$target.v7-stage"
$backup = "$target.v7-backup"
foreach ($candidate in @($stage, $backup)) {
    $full = [IO.Path]::GetFullPath($candidate)
    if (-not $full.StartsWith($installRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Local install working path escaped ${installRoot}: $full"
    }
}
if (Test-Path -LiteralPath $stage) {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
if (Test-Path -LiteralPath $backup) {
    throw "Previous local rollback image still exists: $backup"
}

New-Item -ItemType Directory -Force -Path (Split-Path $target -Parent) | Out-Null
Copy-Item -LiteralPath $source -Destination $stage -Recurse -Force
# jpackage marks launchers and runtime files read-only. Normalize the staged
# image so a later transactional upgrade can test and replace it normally.
Get-ChildItem -LiteralPath $stage -Recurse -File -Force | ForEach-Object {
    if ($_.IsReadOnly) { $_.IsReadOnly = $false }
}
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'extensions') | Out-Null
if ($packagedExtensionsReady -eq 0) {
    Copy-Item -LiteralPath (Join-Path $packagedExtensionRoot 'HLSDownloader-7.0.0-Chromium.zip') -Destination (Join-Path $stage 'extensions\HLSDownloader-7.0.0-Chromium.zip') -Force
    Copy-Item -LiteralPath (Join-Path $packagedExtensionRoot 'HLSDownloader-7.0.0-Firefox.zip') -Destination (Join-Path $stage 'extensions\HLSDownloader-7.0.0-Firefox.zip') -Force
} else {
    Compress-Archive -Path (Join-Path $extensionRoot 'chrome-mv3\*') -DestinationPath (Join-Path $stage 'extensions\HLSDownloader-7.0.0-Chromium.zip') -CompressionLevel Optimal -Force
    Compress-Archive -Path (Join-Path $extensionRoot 'firefox-mv3\*') -DestinationPath (Join-Path $stage 'extensions\HLSDownloader-7.0.0-Firefox.zip') -CompressionLevel Optimal -Force
}
Copy-Item -LiteralPath $note -Destination (Join-Path $stage 'HLS-Downloader-7.0.0-升级说明.md') -Force
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'scripts') | Out-Null
Copy-Item -LiteralPath (Join-Path $repo 'scripts\upgrade-v7-portable.ps1') -Destination (Join-Path $stage 'scripts\upgrade-v7-portable.ps1') -Force

$sourcePortable = Join-Path $repo 'artifacts\v7-productization\package\HLSDownloader-7.0.0-Windows-x64-Portable.zip'
$portableHash = if (Test-Path -LiteralPath $sourcePortable) {
    (Get-FileHash -LiteralPath $sourcePortable -Algorithm SHA256).Hash
} else {
    'not-recorded'
}
$installInfo = @(
    'HLS Downloader 7.0.0',
    "InstalledAt=$([DateTimeOffset]::Now.ToString('o'))",
    "SourcePortableSHA256=$portableHash",
    "ExecutableSHA256=$((Get-FileHash -LiteralPath (Join-Path $stage 'HLSDownloader.exe') -Algorithm SHA256).Hash)"
) -join "`r`n"
[IO.File]::WriteAllText((Join-Path $stage 'INSTALLATION.txt'), $installInfo, [Text.UTF8Encoding]::new($false))

$hadPrevious = Test-Path -LiteralPath $target
try {
    if ($hadPrevious) {
        Move-Item -LiteralPath $target -Destination $backup
    }
    Move-Item -LiteralPath $stage -Destination $target
} catch {
    if (Test-Path -LiteralPath $target) {
        Remove-Item -LiteralPath $target -Recurse -Force
    }
    if ($hadPrevious -and (Test-Path -LiteralPath $backup)) {
        Move-Item -LiteralPath $backup -Destination $target
    }
    throw
}

$engineExecutable = Join-Path $target 'app\resources\HLSDownloaderEngine.exe'
$hostExecutable = Join-Path $target 'app\resources\HLSDownloaderNativeHost.exe'
$desktop = [Environment]::GetFolderPath('Desktop')
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\HLS Downloader'
$desktopShortcut = Join-Path $desktop 'HLS Downloader 7.0.0.lnk'
$desktopExtensionPaths = @{
    Chromium = Join-Path $desktop 'HLSDownloader-Chromium.zip'
    Firefox = Join-Path $desktop 'HLSDownloader-Firefox.zip'
}
try {
    $registration = Start-Process -FilePath $engineExecutable -ArgumentList '--register-native-host' -NoNewWindow -Wait -PassThru
    if ($registration.ExitCode -ne 0) {
        throw "v7 Native Host registration failed with exit $($registration.ExitCode)"
    }

    $shell = New-Object -ComObject WScript.Shell
    New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
    $shortcut = $shell.CreateShortcut((Join-Path $startMenu 'HLS Downloader 7.0.0.lnk'))
    $shortcut.TargetPath = Join-Path $target 'HLSDownloader.exe'
    $shortcut.WorkingDirectory = $target
    $shortcut.IconLocation = (Join-Path $target 'HLSDownloader.exe') + ',0'
    $shortcut.Description = 'HLS Downloader 7.0.0'
    $shortcut.Save()

    $desktopLink = $shell.CreateShortcut($desktopShortcut)
    $desktopLink.TargetPath = Join-Path $target 'HLSDownloader.exe'
    $desktopLink.WorkingDirectory = $target
    $desktopLink.IconLocation = (Join-Path $target 'HLSDownloader.exe') + ',0'
    $desktopLink.Description = 'HLS Downloader 7.0.0'
    $desktopLink.Save()

    # Keep exactly one current extension package per browser on the desktop.
    foreach ($browser in @('Chromium', 'Firefox')) {
        Get-ChildItem -LiteralPath $desktop -Filter "HLSDownloader-*${browser}.zip" -File -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
        Copy-Item -LiteralPath (Join-Path $target "extensions\HLSDownloader-7.0.0-$browser.zip") `
            -Destination $desktopExtensionPaths[$browser] -Force
    }
} catch {
    if (Test-Path -LiteralPath $target) {
        Remove-Item -LiteralPath $target -Recurse -Force
    }
    if ($hadPrevious -and (Test-Path -LiteralPath $backup)) {
        Move-Item -LiteralPath $backup -Destination $target
    }
    throw
}
if ($hadPrevious -and (Test-Path -LiteralPath $backup)) {
    Remove-Item -LiteralPath $backup -Recurse -Force
}

[ordered]@{
    installed = $true
    version = '7.0.0'
    target = $target
    rollback = ''
    native_host = $hostExecutable
    chromium_extension = Join-Path $target 'extensions\HLSDownloader-7.0.0-Chromium.zip'
    firefox_extension = Join-Path $target 'extensions\HLSDownloader-7.0.0-Firefox.zip'
    start_menu = Join-Path $startMenu 'HLS Downloader 7.0.0.lnk'
    desktop = $desktopShortcut
} | ConvertTo-Json -Depth 3
