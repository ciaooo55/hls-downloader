[CmdletBinding(SupportsShouldProcess)]
param(
    [switch]$Apply
)

$ErrorActionPreference = 'Stop'
$programsRoot = [IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs')).TrimEnd([IO.Path]::DirectorySeparatorChar)
$legacyPrograms = [IO.Path]::GetFullPath((Join-Path $programsRoot 'HLS Downloader v6')).TrimEnd([IO.Path]::DirectorySeparatorChar)
$legacyStandalone = [IO.Path]::GetFullPath('E:\HLS Downloader').TrimEnd([IO.Path]::DirectorySeparatorChar)
$startRoot = [IO.Path]::GetFullPath((Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs')).TrimEnd([IO.Path]::DirectorySeparatorChar)
$namedShortcutPaths = @(
    (Join-Path $startRoot 'HLS Downloader v6.lnk'),
    (Join-Path $startRoot 'HLS Downloader\HLS Downloader.lnk')
)

if (-not $legacyPrograms.StartsWith($programsRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Legacy per-user install escaped Programs: $legacyPrograms"
}
if ($legacyStandalone -ne [IO.Path]::GetFullPath('E:\HLS Downloader').TrimEnd([IO.Path]::DirectorySeparatorChar)) {
    throw "Unexpected standalone legacy install path: $legacyStandalone"
}
foreach ($shortcut in $namedShortcutPaths) {
    $resolved = [IO.Path]::GetFullPath($shortcut)
    if (-not $resolved.StartsWith($startRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Legacy shortcut escaped Start Menu: $resolved"
    }
}

$installTargets = @($legacyPrograms, $legacyStandalone) | Where-Object { Test-Path -LiteralPath $_ -PathType Container }
foreach ($target in $installTargets) {
    $userData = @(Get-ChildItem -LiteralPath $target -File -Recurse -Force -ErrorAction SilentlyContinue | Where-Object {
        $_.Name -in @('config.json', 'data.db', 'data.db-shm', 'data.db-wal') -or
        $_.FullName.IndexOf([IO.Path]::DirectorySeparatorChar + 'downloads' + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $_.FullName.IndexOf([IO.Path]::DirectorySeparatorChar + '.v6-tasks' + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -ge 0
    })
    if ($userData.Count -gt 0) {
        throw "Legacy install contains user data and was not removed: $target"
    }
}
$shell = New-Object -ComObject WScript.Shell
$targetedShortcuts = @(Get-ChildItem -LiteralPath $startRoot -File -Filter '*.lnk' -Recurse -Force -ErrorAction SilentlyContinue | Where-Object {
    $target = [string]$shell.CreateShortcut($_.FullName).TargetPath
    $target.StartsWith($legacyPrograms + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
    $target.StartsWith($legacyStandalone + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)
} | ForEach-Object { $_.FullName })
$existingShortcuts = @($namedShortcutPaths + $targetedShortcuts | Where-Object {
    Test-Path -LiteralPath $_ -PathType Leaf
} | Sort-Object -Unique)

if (-not $Apply) {
    Write-Output "DRY-RUN: $($installTargets.Count) legacy installs and $($existingShortcuts.Count) stale shortcuts; no file changed."
    exit 0
}
foreach ($target in $installTargets) { Remove-Item -LiteralPath $target -Recurse -Force }
foreach ($shortcut in $existingShortcuts) { Remove-Item -LiteralPath $shortcut -Force }
Write-Output "REMOVED_LEGACY_INSTALLS=$($installTargets.Count)"
Write-Output "REMOVED_STALE_SHORTCUTS=$($existingShortcuts.Count)"
