[CmdletBinding()]
param(
    [string]$SourceDir = 'D:\HLSDownloaderBuildCache\compose-build\compose\binaries\main\app\HLSDownloader',
    [string]$TargetDir = '',
    [string]$ExtensionOutput = '',
    [string]$UpgradeNote = ''
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$programsRoot = [IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs')).TrimEnd('\', '/')
$source = [IO.Path]::GetFullPath($SourceDir).TrimEnd('\', '/')
$target = if ([String]::IsNullOrWhiteSpace($TargetDir)) {
    Join-Path $programsRoot 'HLSDownloader'
} else {
    [IO.Path]::GetFullPath($TargetDir).TrimEnd('\', '/')
}
$targetPrefix = $programsRoot + [IO.Path]::DirectorySeparatorChar
if (-not $target.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Local install target must stay under ${programsRoot}: $target"
}
foreach ($name in @(
    'HLSDownloader.exe',
    'app',
    'runtime',
    'app\resources\HLSDownloaderEngine.exe',
    'app\resources\HLSDownloaderNativeHost.exe',
    'app\resources\HLSDownloaderPresenter.exe',
    'app\resources\ffmpeg.exe',
    'app\resources\ffprobe.exe',
    'app\resources\libmpv-2.dll'
)) {
    if (-not (Test-Path -LiteralPath (Join-Path $source $name))) {
        throw "v7 local image is incomplete; missing ${name}: $source"
    }
}

$extensionRoot = if ([String]::IsNullOrWhiteSpace($ExtensionOutput)) {
    Join-Path $repo 'extension\.output'
} else {
    [IO.Path]::GetFullPath($ExtensionOutput)
}
$note = if ([String]::IsNullOrWhiteSpace($UpgradeNote)) {
    Join-Path $repo 'docs\v7-desktop-upgrade-note.md'
} else {
    [IO.Path]::GetFullPath($UpgradeNote)
}
foreach ($browser in @('chrome-mv3', 'firefox-mv3')) {
    if (-not (Test-Path -LiteralPath (Join-Path $extensionRoot $browser) -PathType Container)) {
        throw "Production browser extension is missing: $browser"
    }
}
if (-not (Test-Path -LiteralPath $note -PathType Leaf)) {
    throw "Upgrade note is missing: $note"
}

$stage = "$target.v7-stage"
$backup = "$target.v7-backup"
foreach ($candidate in @($stage, $backup)) {
    $full = [IO.Path]::GetFullPath($candidate)
    if (-not $full.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Local install working path escaped Programs: $full"
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
Compress-Archive -Path (Join-Path $extensionRoot 'chrome-mv3\*') -DestinationPath (Join-Path $stage 'extensions\HLSDownloader-7.0.0-Chromium.zip') -CompressionLevel Optimal -Force
Compress-Archive -Path (Join-Path $extensionRoot 'firefox-mv3\*') -DestinationPath (Join-Path $stage 'extensions\HLSDownloader-7.0.0-Firefox.zip') -CompressionLevel Optimal -Force
Copy-Item -LiteralPath $note -Destination (Join-Path $stage 'HLS-Downloader-7.0.0-升级说明.md') -Force
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'scripts') | Out-Null
Copy-Item -LiteralPath (Join-Path $repo 'scripts\upgrade-v7-portable.ps1') -Destination (Join-Path $stage 'scripts\upgrade-v7-portable.ps1') -Force
Copy-Item -LiteralPath (Join-Path $repo 'scripts\register-v7-native-host.ps1') -Destination (Join-Path $stage 'scripts\register-v7-native-host.ps1') -Force

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

$hostExecutable = Join-Path $target 'app\resources\HLSDownloaderNativeHost.exe'
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repo 'scripts\register-v7-native-host.ps1') -HostExecutable $hostExecutable
if ($LASTEXITCODE -ne 0) {
    throw "v7 Native Host registration failed with exit $LASTEXITCODE"
}

$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\HLS Downloader'
New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut((Join-Path $startMenu 'HLS Downloader 7.0.0.lnk'))
$shortcut.TargetPath = Join-Path $target 'HLSDownloader.exe'
$shortcut.WorkingDirectory = $target
$shortcut.IconLocation = (Join-Path $target 'HLSDownloader.exe') + ',0'
$shortcut.Description = 'HLS Downloader 7.0.0'
$shortcut.Save()

$desktopShortcut = Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLS Downloader 7.0.0.lnk'
$desktopLink = $shell.CreateShortcut($desktopShortcut)
$desktopLink.TargetPath = Join-Path $target 'HLSDownloader.exe'
$desktopLink.WorkingDirectory = $target
$desktopLink.IconLocation = (Join-Path $target 'HLSDownloader.exe') + ',0'
$desktopLink.Description = 'HLS Downloader 7.0.0'
$desktopLink.Save()

[ordered]@{
    installed = $true
    version = '7.0.0'
    target = $target
    rollback = if ($hadPrevious) { $backup } else { '' }
    native_host = $hostExecutable
    chromium_extension = Join-Path $target 'extensions\HLSDownloader-7.0.0-Chromium.zip'
    firefox_extension = Join-Path $target 'extensions\HLSDownloader-7.0.0-Firefox.zip'
    start_menu = Join-Path $startMenu 'HLS Downloader 7.0.0.lnk'
    desktop = $desktopShortcut
} | ConvertTo-Json -Depth 3
