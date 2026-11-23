[CmdletBinding()]
param(
    [string]$ManifestPath = 'artifacts\v7-productization\package\ARTIFACT-MANIFEST.json',
    [string]$ReleaseRoot = 'artifacts\v7-productization\release-staging',
    [string]$FirefoxSources = '',
    [string]$Python = $(if ($env:HLS_V7_PYTHON) { $env:HLS_V7_PYTHON } else { 'python.exe' })
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\V7HashFunctions.ps1')
Import-Module (Join-Path $PSScriptRoot 'V7ReleaseScope.psm1') -Force
Add-Type -AssemblyName System.IO.Compression.FileSystem
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$manifestFullPath = if ([IO.Path]::IsPathRooted($ManifestPath)) { [IO.Path]::GetFullPath($ManifestPath) } else { [IO.Path]::GetFullPath((Join-Path $repo $ManifestPath)) }
$manifest = Get-Content -LiteralPath $manifestFullPath -Raw -Encoding UTF8 | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
if ([int]$manifest.schema -ne 1 -or [string]$manifest.package_tier -ne 'formal' -or [string]$manifest.source_commit -ne $currentCommit -or [string]$manifest.source_tree -ne $currentTree) {
    throw 'Release staging requires a formal package from the current source commit/tree.'
}
$version = [string]$manifest.product_version
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "Invalid release version: $version" }
$packageRoot = Split-Path $manifestFullPath -Parent
$destinationRoot = if ([IO.Path]::IsPathRooted($ReleaseRoot)) { [IO.Path]::GetFullPath($ReleaseRoot) } else { [IO.Path]::GetFullPath((Join-Path $repo $ReleaseRoot)) }
$destinationRoot = $destinationRoot.TrimEnd('\', '/')
$parent = [IO.Path]::GetDirectoryName($destinationRoot)
if (-not $parent) { throw 'Release staging needs a dedicated directory.' }
$previous = $null
function Assert-StagingDirectory([string]$Directory) {
    $items = @(Get-ChildItem -LiteralPath $Directory -Force)
    if (-not [IO.Directory]::Exists($Directory) -or
        ([IO.File]::GetAttributes($Directory) -band [IO.FileAttributes]::ReparsePoint)) { throw 'Invalid release staging directory.' }
    if ($items.Count -gt 0) {
        $names = @($items | ForEach-Object { $_.Name })
        foreach ($required in @('ARTIFACT-MANIFEST.json', 'FEATURE-PARITY.json', 'RELEASE-EVIDENCE.json', 'SHA256SUMS.txt')) {
            if ($required -notin $names) { throw "Refusing to replace unrelated staging directory: $Directory" }
        }
        foreach ($item in $items) {
            if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
                $item.Name -notmatch '\A(ARTIFACT-MANIFEST\.json|FEATURE-PARITY\.json|RELEASE-EVIDENCE\.json|SHA256SUMS\.txt|README-[0-9.]+\.txt|HLSDownloader-(Extension-[0-9.]+-(Chromium|Firefox|Firefox-Sources)\.zip|[0-9.]+-(Windows-x64\.(exe|msi)|Windows-x64-Portable\.zip|Release-Evidence\.zip|SBOM\.cdx\.json)))\z') {
                throw "Refusing to replace unrelated staging item: $($item.FullName)"
            }
        }
    }
}
if (Test-Path -LiteralPath $destinationRoot) { Assert-StagingDirectory $destinationRoot }
$stage = Join-Path $parent ('.stage-' + [guid]::NewGuid().ToString('n'))
[void][IO.Directory]::CreateDirectory($stage)

function Copy-ReleaseAsset([string]$Source, [string]$Name) {
    $destination = Join-Path $stage $Name
    [IO.File]::Copy($Source, $destination)
    return $destination
}

try {
    $exe = Resolve-V7PackageFile $packageRoot $manifest.artifacts.exe.path $manifest.artifacts.exe.sha256
    $msi = Resolve-V7PackageFile $packageRoot $manifest.artifacts.msi.path $manifest.artifacts.msi.sha256
    $portable = Resolve-V7PackageFile $packageRoot $manifest.artifacts.portable.path $manifest.artifacts.portable.sha256
    [void](Copy-ReleaseAsset $exe "HLSDownloader-$version-Windows-x64.exe")
    [void](Copy-ReleaseAsset $msi "HLSDownloader-$version-Windows-x64.msi")
    [void](Copy-ReleaseAsset $portable "HLSDownloader-$version-Windows-x64-Portable.zip")
    foreach ($browser in @('Chromium', 'Firefox')) {
        $entry = $manifest.extensions.$browser
        $source = Resolve-V7PackageFile $packageRoot $entry.path $entry.sha256
        [void](Copy-ReleaseAsset $source "HLSDownloader-Extension-$version-$browser.zip")
    }
    if (-not [String]::IsNullOrWhiteSpace($FirefoxSources)) {
        $source = (Resolve-Path -LiteralPath $FirefoxSources).Path
        [void](Copy-ReleaseAsset $source "HLSDownloader-Extension-$version-Firefox-Sources.zip")
    }

    $feature = Resolve-V7PackageFile $packageRoot $manifest.feature_parity_path $manifest.feature_parity_sha256
    [void](Copy-ReleaseAsset $feature 'FEATURE-PARITY.json')
    [void](Copy-ReleaseAsset $manifestFullPath 'ARTIFACT-MANIFEST.json')
    $releaseEvidence = Join-Path $repo 'artifacts\v7-productization\release-evidence.json'
    if (-not (Test-Path -LiteralPath $releaseEvidence -PathType Leaf)) { throw 'Release evidence is missing.' }
    [void](Copy-ReleaseAsset $releaseEvidence 'RELEASE-EVIDENCE.json')
    $evidenceDir = Join-Path $repo 'artifacts\v7-productization\release-evidence'
    if (-not (Test-Path -LiteralPath $evidenceDir -PathType Container)) { throw 'Per-gate release evidence is missing.' }
    $gateBundle = Join-Path $stage "HLSDownloader-$version-Release-Evidence.zip"
    [IO.Compression.ZipFile]::CreateFromDirectory($evidenceDir, $gateBundle)

    $sbom = Join-Path $stage "HLSDownloader-$version-SBOM.cdx.json"
    if ([string]$manifest.ui_runtime -ne 'native') { throw 'Release requires the native UI runtime.' }
    $uiRuntime = 'native'
    & $Python (Join-Path $PSScriptRoot 'generate_sbom.py') --version $version --output $sbom --ui-runtime $uiRuntime
    if ($LASTEXITCODE -ne 0) { throw "SBOM generation failed with exit code $LASTEXITCODE." }

    $notesPath = Join-Path $stage "README-$version.txt"
    $notes = @"
HLS Downloader $version

Source commit: $currentCommit
Source tree: $currentTree

This release was built from one frozen source commit. Executed gates and explicit user exclusions are recorded separately in RELEASE-EVIDENCE.json and FEATURE-PARITY.json. Real casting/TVBox and account-dependent cloud subtitle quality were excluded by the operator; their exclusion is not a passed test. Windows release binaries are unsigned. Install updates manually; the automatic updater requires a trusted signature. Verify every downloaded file against SHA256SUMS.txt before redistribution.

Firefox extension ZIP: unsigned package for temporary loading and subsequent Mozilla signing. Permanent installation in standard Firefox requires a signed XPI. This release does not claim that signing or permanent-install acceptance was completed.
"@
    $changesPath = Join-Path $repo "docs\releases\v$version.md"
    if (Test-Path -LiteralPath $changesPath -PathType Leaf) {
        $notes += "`r`n`r`n" + [IO.File]::ReadAllText($changesPath, [Text.Encoding]::UTF8).Trim()
    }
    [IO.File]::WriteAllText($notesPath, $notes.Trim() + "`r`n", [Text.UTF8Encoding]::new($false))

    $checksumPath = Join-Path $stage 'SHA256SUMS.txt'
    $checksumLines = @(Get-ChildItem -LiteralPath $stage -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | Sort-Object Name | ForEach-Object {
        "$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant())  $($_.Name)"
    })
    [IO.File]::WriteAllLines($checksumPath, $checksumLines, [Text.UTF8Encoding]::new($false))
    if ([IO.Directory]::Exists($destinationRoot)) {
        $previous = Join-Path $parent ('.previous-stage-' + [guid]::NewGuid().ToString('n'))
        [IO.Directory]::Move($destinationRoot, $previous)
    }
    try {
        if ($previous) { Assert-StagingDirectory $previous }
        [IO.Directory]::Move($stage, $destinationRoot)
    } catch {
        if ($previous) { [IO.Directory]::Move($previous, $destinationRoot) }
        throw
    }
    if ($previous) { Remove-Item -LiteralPath $previous -Recurse -Force }
} finally {
    if ([IO.Directory]::Exists($stage)) { Remove-Item -LiteralPath $stage -Recurse -Force }
}
Write-Output ([ordered]@{ schema = 1; version = $version; source_commit = $currentCommit; release_root = $destinationRoot; asset_count = @(Get-ChildItem -LiteralPath $destinationRoot -File).Count } | ConvertTo-Json -Compress)
