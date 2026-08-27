[CmdletBinding(SupportsShouldProcess)]
param(
    [switch]$Apply,
    [switch]$LegacyCargoTargetOnly,
    [switch]$NativeHostSmokeOnly,
    [switch]$ExtensionAdversarialOnly,
    [switch]$ProductizationJunkOnly,
    [switch]$BuildOutputsOnly,
    [switch]$StaleSmokeOnly,
    [switch]$ConsolidateArchives,
    [string]$ArchiveRoot = 'D:\HLSDownloader-archives',
    [string]$KeepArchiveName = 'v7.0.0-verified-20260824',
    [string]$ReportRoot = ''
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
$reportRootPath = if([String]::IsNullOrWhiteSpace($ReportRoot)) { Join-Path $repo 'artifacts\v7-implementation' } else { $ReportRoot }
$reportRootPath = [IO.Path]::GetFullPath($reportRootPath)
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$archive = Join-Path $ArchiveRoot "pre-v7-$stamp"
New-Item -ItemType Directory -Force -Path $reportRootPath | Out-Null
if ($NativeHostSmokeOnly) {
    $targets = @(Get-ChildItem -LiteralPath ([IO.Path]::GetTempPath()) -Directory -Filter 'hls-v7-native-host-*' -ErrorAction SilentlyContinue |
        ForEach-Object { $_.FullName })
} elseif ($ExtensionAdversarialOnly) {
    $targets = @(
        (Join-Path $repo 'extension\node_modules'),
        'D:\HLSDownloaderBuildCache\pnpm-adversarial-store'
    )
} elseif ($ProductizationJunkOnly) {
    # Superseded local binaries and one-off audit copies. Keep the current
    # candidate, transaction evidence, source-controlled matrix and user data.
    $targets = @(
        (Join-Path $repo 'artifacts\v7-productization\release'),
        (Join-Path $repo 'artifacts\v7-productization\package'),
        (Join-Path $repo 'artifacts\v7-productization\tar-smoke-53b72cb0c8384cc1866906b77e2cabec'),
        (Join-Path $repo 'artifacts\v7-productization\popup-white-fix'),
        (Join-Path $repo 'artifacts\v7-productization\tray-shortcut-residency')
    )
} elseif ($BuildOutputsOnly) {
    # Delete compiled outputs and one-off audit workspaces. Keep downloaded
    # toolchains/dependency caches and all productization evidence.
    $targets = @(
        (Join-Path $repo 'presenter_ui\target'),
        (Join-Path $repo 'native_shell\target'),
        (Join-Path $repo 'target'),
        (Join-Path $repo 'build'),
        (Join-Path $repo 'desktop_ui\build'),
        (Join-Path $repo 'desktop_ui\.gradle'),
        (Join-Path $repo 'desktop_ui\.kotlin'),
        (Join-Path $repo 'extension\dist'),
        (Join-Path $repo 'extension\.output'),
        (Join-Path $repo 'extension\.wxt'),
        (Join-Path $repo 'extension\node_modules'),
        'D:\HLSDownloaderBuildCache\cargo-target',
        'D:\HLSDownloaderBuildCache\target-native',
        'D:\HLSDownloaderBuildCache\compose-build',
        'D:\HLSDownloaderBuildCache\gradle-home',
        'D:\HLSDownloaderBuildCache\v3-source-audit',
        'D:\HLSDownloaderBuildCache\pnpm-store-v3-audit',
        'D:\HLSDownloaderBuildCache\v3-full-audit-data'
    )
} elseif ($StaleSmokeOnly) {
    $cutoff = (Get-Date).Date
    $targets = @(Get-ChildItem -LiteralPath ([IO.Path]::GetTempPath()) -Directory -Filter 'hls-v7-*' -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -lt $cutoff } |
        ForEach-Object { $_.FullName })
} else {
    $relativeTargets = if ($LegacyCargoTargetOnly) {
        @('native_shell\target')
    } else {
        @(
            'presenter_ui\target',
            'native_shell\target',
            'native_shell\site-cache',
            'target',
            'build',
            'desktop_ui\build',
            'desktop_ui\.gradle',
            'desktop_ui\.kotlin',
            'desktop_ui\resources\HLSDownloaderEngine.exe',
            'desktop_ui\resources\common\HLSDownloaderEngine.exe',
            'desktop_ui\resources\common\HLSDownloaderNativeHost.exe',
            'desktop_ui\resources\common\HLSDownloaderUpdater.exe',
            'desktop_ui\resources\common\HLSDownloaderPresenter.exe',
            'extension\dist',
            'extension\.output',
            'extension\.wxt',
            'extension\node_modules',
            'artifacts\v7-implementation',
            'artifacts\v7-productization\fixtures',
            'artifacts\v7-productization\installed',
            'artifacts\v7-productization\package',
            'artifacts\v7-productization\performance',
            'artifacts\v7-productization\runtime-current',
            'artifacts\v7-productization\runtime-test',
            'artifacts\v7-productization\ui-api',
            'artifacts\v7-productization\ui-user-review',
            '.coverage',
            '.mypy_cache',
            '.pytest_cache',
            '.ruff_cache'
        )
    }
    $targets = @($relativeTargets | ForEach-Object { Join-Path $repo $_ }) + @(
        'D:\HLSDownloaderBuildCache\ab-reference',
        'D:\HLSDownloaderBuildCache\cargo-target',
        'D:\HLSDownloaderBuildCache\compose-build',
        'E:\HLSDownloaderBuildCache\cargo',
        'E:\HLSDownloaderBuildCache\gradle',
        'E:\HLSDownloaderBuildCache\gradle-9.7.0',
        'E:\HLSDownloaderBuildCache\gradle-9.7.0-bin.zip',
        'E:\HLSDownloaderBuildCache\libmpv-20260814',
        'E:\HLSDownloaderBuildCache\compose-after-click.png',
        'E:\HLSDownloaderBuildCache\compose-after-resolution.png',
        'E:\HLSDownloaderBuildCache\compose-handoff-contrast.png',
        'E:\HLSDownloaderBuildCache\compose-handoff.png',
        'E:\HLSDownloaderBuildCache\compose-reconciled.png',
        'E:\HLSDownloaderBuildCache\compose-workbench.png'
    )
    $targets += @(Get-ChildItem -LiteralPath $repo -Directory -Filter '__pycache__' -Recurse -ErrorAction SilentlyContinue |
        ForEach-Object { $_.FullName })
}
$scopedCleanup = $NativeHostSmokeOnly -or $ExtensionAdversarialOnly -or $ProductizationJunkOnly -or $BuildOutputsOnly -or $StaleSmokeOnly
$release = if ($scopedCleanup) {
    $null
} else {
    Join-Path $repo 'release'
}
$records = @(foreach($path in $targets) {
    if(Test-Path -LiteralPath $path) {
        $bytes = (Get-ChildItem -LiteralPath $path -Force -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
        [pscustomobject]@{ path=$path; bytes=[int64]$bytes; action='delete-rebuildable'; sha256=$null }
    }
})
if($null -ne $release -and (Test-Path -LiteralPath $release)) {
    Get-ChildItem -LiteralPath $release -File -Recurse | ForEach-Object {
        $sha=[System.Security.Cryptography.SHA256]::Create(); $stream=[IO.File]::OpenRead($_.FullName)
        try { $hash=([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-','') } finally { $stream.Dispose(); $sha.Dispose() }
        $records += [pscustomobject]@{ path=$_.FullName; bytes=[int64]$_.Length; action='archive-release'; sha256=$hash }
    }
}
$before = Join-Path $reportRootPath 'cleanup-before.json'
[IO.File]::WriteAllText($before, ($records | ConvertTo-Json -Depth 4), [Text.UTF8Encoding]::new($false))
if(!$Apply) { Write-Output "DRY-RUN: $($records.Count) entries; no file changed. Report: $before"; exit 0 }
if ($ConsolidateArchives) {
    $resolvedArchiveRoot = [IO.Path]::GetFullPath($ArchiveRoot).TrimEnd([IO.Path]::DirectorySeparatorChar)
    $keepArchive = [IO.Path]::GetFullPath((Join-Path $resolvedArchiveRoot $KeepArchiveName)).TrimEnd([IO.Path]::DirectorySeparatorChar)
    if (-not $keepArchive.StartsWith($resolvedArchiveRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Archive keep path escaped archive root: $keepArchive"
    }
    if (-not (Test-Path -LiteralPath $keepArchive -PathType Container)) {
        throw "Verified rollback archive is missing: $keepArchive"
    }
    $oldArchives = @(Get-ChildItem -LiteralPath $resolvedArchiveRoot -Force | Where-Object {
        [IO.Path]::GetFullPath($_.FullName).TrimEnd([IO.Path]::DirectorySeparatorChar) -ne $keepArchive
    })
    foreach ($item in $oldArchives) {
        $resolved = [IO.Path]::GetFullPath($item.FullName)
        if (-not $resolved.StartsWith($resolvedArchiveRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Archive cleanup target escaped archive root: $resolved"
        }
    }
    foreach ($item in $oldArchives) { Remove-Item -LiteralPath $item.FullName -Recurse -Force }
    Write-Output "ARCHIVE_KEEP=$keepArchive"
    Write-Output "REMOVED_ARCHIVE_ENTRIES=$($oldArchives.Count)"
}
foreach($file in @($records | Where-Object action -eq 'archive-release')) {
    $relative = [IO.Path]::GetRelativePath($release, $file.path)
    $destination = Join-Path (Join-Path $archive 'release') $relative
    New-Item -ItemType Directory -Force -Path (Split-Path $destination -Parent) | Out-Null
    Move-Item -LiteralPath $file.path -Destination $destination -Force
}
foreach($entry in @($records | Where-Object action -eq 'delete-rebuildable')) { if(Test-Path -LiteralPath $entry.path) { Remove-Item -LiteralPath $entry.path -Recurse -Force } }
$after = foreach($record in $records) { [pscustomobject]@{ path=$record.path; action=$record.action; exists=(Test-Path -LiteralPath $record.path); bytes=$record.bytes; sha256=$record.sha256 } }
$diff = Join-Path $reportRootPath 'cleanup-diff.txt'
$archivedCount = @($records | Where-Object action -eq 'archive-release').Count
[IO.File]::WriteAllText($diff, (($after | Format-Table -AutoSize | Out-String) + "`nArchived release files: $archivedCount`nArchive: $archive`n"), [Text.UTF8Encoding]::new($false))
Write-Output "CLEANED: $($records.Count) entries; archived release files: $archivedCount; report: $diff"
