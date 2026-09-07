[CmdletBinding()]
param(
    [string]$ManifestPath = 'artifacts\v7-productization\package\ARTIFACT-MANIFEST.json',
    [string]$ReleaseRoot = 'artifacts\v7-productization\release-staging',
    [string]$FirefoxSources = '',
    [string]$Python = $(if ($env:HLS_V7_PYTHON) { $env:HLS_V7_PYTHON } else { 'python.exe' })
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
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
$stage = if ([IO.Path]::IsPathRooted($ReleaseRoot)) { [IO.Path]::GetFullPath($ReleaseRoot) } else { [IO.Path]::GetFullPath((Join-Path $repo $ReleaseRoot)) }
Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null

function Resolve-VerifiedArtifact($Entry) {
    $full = [IO.Path]::GetFullPath((Join-Path $packageRoot ([string]$Entry.path)))
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Formal artifact is missing: $full" }
    $actual = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne ([string]$Entry.sha256).ToLowerInvariant()) { throw "Formal artifact SHA-256 mismatch: $full" }
    return $full
}
function Copy-ReleaseAsset([string]$Source, [string]$Name) {
    $destination = Join-Path $stage $Name
    Copy-Item -LiteralPath $Source -Destination $destination -Force
    return $destination
}

$exe = Resolve-VerifiedArtifact $manifest.artifacts.exe
$msi = Resolve-VerifiedArtifact $manifest.artifacts.msi
$portable = Resolve-VerifiedArtifact $manifest.artifacts.portable
[void](Copy-ReleaseAsset $exe "HLSDownloader-$version-Windows-x64.exe")
[void](Copy-ReleaseAsset $msi "HLSDownloader-$version-Windows-x64.msi")
[void](Copy-ReleaseAsset $portable "HLSDownloader-$version-Windows-x64-Portable.zip")
foreach ($browser in @('Chromium', 'Firefox')) {
    $entry = $manifest.extensions.$browser
    $source = Resolve-VerifiedArtifact $entry
    [void](Copy-ReleaseAsset $source "HLSDownloader-Extension-$version-$browser.zip")
}
if (-not [String]::IsNullOrWhiteSpace($FirefoxSources)) {
    $source = (Resolve-Path -LiteralPath $FirefoxSources).Path
    [void](Copy-ReleaseAsset $source "HLSDownloader-Extension-$version-Firefox-Sources.zip")
}

foreach ($name in @('FEATURE-PARITY.json', 'ARTIFACT-MANIFEST.json', 'AUTHENTICODE-SIGNATURES.json')) {
    $source = if ($name -eq 'FEATURE-PARITY.json') { Join-Path $packageRoot $name } elseif ($name -eq 'ARTIFACT-MANIFEST.json') { $manifestFullPath } else { Join-Path $packageRoot $name }
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Required release metadata is missing: $source" }
    [void](Copy-ReleaseAsset $source $name)
}
$releaseEvidence = Join-Path $repo 'artifacts\v7-productization\release-evidence.json'
if (-not (Test-Path -LiteralPath $releaseEvidence -PathType Leaf)) { throw 'Release evidence is missing.' }
[void](Copy-ReleaseAsset $releaseEvidence 'RELEASE-EVIDENCE.json')
$evidenceDir = Join-Path $repo 'artifacts\v7-productization\release-evidence'
if (-not (Test-Path -LiteralPath $evidenceDir -PathType Container)) { throw 'Per-gate release evidence is missing.' }
$gateBundle = Join-Path $stage "HLSDownloader-$version-Release-Evidence.zip"
Compress-Archive -Path (Join-Path $evidenceDir '*.json') -DestinationPath $gateBundle -CompressionLevel Optimal -Force

$sbom = Join-Path $stage "HLSDownloader-$version-SBOM.cdx.json"
& $Python (Join-Path $PSScriptRoot 'generate_sbom.py') --version $version --output $sbom
if ($LASTEXITCODE -ne 0) { throw "SBOM generation failed with exit code $LASTEXITCODE." }

$notesPath = Join-Path $stage "README-$version.txt"
$notes = @"
HLS Downloader $version

Source commit: $currentCommit
Source tree: $currentTree

This release was built from one frozen source commit. Browser, performance, installer-upgrade and rollback gates are recorded in RELEASE-EVIDENCE.json and the evidence ZIP. Windows release binaries are Authenticode signed and timestamped; AUTHENTICODE-SIGNATURES.json records the signer used by the release runner. Verify every downloaded file against SHA256SUMS.txt before redistribution.
"@
[IO.File]::WriteAllText($notesPath, $notes.Trim() + "`r`n", [Text.UTF8Encoding]::new($false))

$checksumPath = Join-Path $stage 'SHA256SUMS.txt'
$checksumLines = @(Get-ChildItem -LiteralPath $stage -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' } | Sort-Object Name | ForEach-Object {
    "$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant())  $($_.Name)"
})
[IO.File]::WriteAllLines($checksumPath, $checksumLines, [Text.UTF8Encoding]::new($false))
Write-Output ([ordered]@{ schema = 1; version = $version; source_commit = $currentCommit; release_root = $stage; asset_count = @(Get-ChildItem -LiteralPath $stage -File).Count } | ConvertTo-Json -Compress)
