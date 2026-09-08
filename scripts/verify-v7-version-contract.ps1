[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Import-Module (Join-Path $PSScriptRoot 'V7VersionContract.psm1') -Force

function Read-Utf8([string]$RelativePath) {
    [IO.File]::ReadAllText((Join-Path $repo $RelativePath), [Text.Encoding]::UTF8)
}

function Match-One([string]$RelativePath, [string]$Pattern, [string]$Label) {
    $matches = [regex]::Matches((Read-Utf8 $RelativePath), $Pattern)
    if ($matches.Count -ne 1) {
        throw "${Label}: expected exactly one version match in $RelativePath, found $($matches.Count)."
    }
    return $matches[0].Groups[1].Value
}

function Assert-Version([string]$Expected, [string]$Actual, [string]$Label) {
    if ($Actual -ne $Expected) {
        throw "$Label version mismatch: expected $Expected, found $Actual. Run scripts\bump-v7-version.ps1 -Version $Expected to repair synchronized version contracts."
    }
}

$featurePath = 'artifacts\v7-productization\feature-parity.json'
$feature = Read-Utf8 $featurePath | ConvertFrom-Json
$version = [string]$feature.product_version
if ($version -notmatch '^\d+\.\d+\.\d+$') {
    throw "feature-parity product_version is not a semantic patch version: $version"
}

Assert-Version $version (Match-One 'native_shell\Cargo.toml' '(?m)^version = "(\d+\.\d+\.\d+)"$' 'Rust Core manifest') 'Rust Core manifest'
Assert-Version $version (Match-One 'presenter_ui\Cargo.toml' '(?m)^version = "(\d+\.\d+\.\d+)"$' 'Presenter manifest') 'Presenter manifest'
Assert-Version $version (Match-One 'extension\package.json' '(?m)^  "version": "(\d+\.\d+\.\d+)",$' 'Extension package') 'Extension package'
Assert-Version $version (Match-One 'desktop_ui\src\main\kotlin\com\hlsdownloader\desktop\Protocol.kt' '(?m)^    const val version = "(\d+\.\d+\.\d+)"$' 'Compose Product.version') 'Compose Product.version'
Assert-Version $version (Match-One 'desktop_ui\src\test\kotlin\com\hlsdownloader\desktop\ProtocolTest.kt' '(?m)^        assertEquals\("(\d+\.\d+\.\d+)", Product\.version\)$' 'Compose version contract test') 'Compose version contract test'

$gradle = Read-Utf8 'desktop_ui\build.gradle.kts'
foreach ($check in @(
    @{ Pattern = '(?m)^version = "(\d+\.\d+\.\d+)"$'; Label = 'Compose project version' },
    @{ Pattern = '(?m)^        packageVersion = "(\d+\.\d+\.\d+)"$'; Label = 'Compose packageVersion' },
    @{ Pattern = '(?m)^        description = "HLS Downloader (\d+\.\d+\.\d+)"$'; Label = 'Compose package description' }
)) {
    $matches = [regex]::Matches($gradle, $check.Pattern)
    if ($matches.Count -ne 1) { throw "$($check.Label): expected exactly one match, found $($matches.Count)." }
    Assert-Version $version $matches[0].Groups[1].Value $check.Label
}

function Assert-LockPackage([string]$RelativePath, [string]$PackageName) {
    $escaped = [regex]::Escape($PackageName)
    $pattern = "(?ms)\[\[package\]\]\r?\nname = `"$escaped`"\r?\nversion = `"(\d+\.\d+\.\d+)`""
    Assert-Version $version (Match-One $RelativePath $pattern "$RelativePath $PackageName") "$RelativePath $PackageName"
}

Assert-LockPackage 'native_shell\Cargo.lock' 'hls-native-shell'
Assert-LockPackage 'presenter_ui\Cargo.lock' 'hls-native-shell'
Assert-LockPackage 'presenter_ui\Cargo.lock' 'hls-native-ui'

$expectedAuditPrefix = 'v' + ($version -replace '\.', '_') + '_'
if (-not ([string]$feature.audit_state).StartsWith($expectedAuditPrefix, [StringComparison]::Ordinal)) {
    throw "feature-parity audit_state must be version-scoped with prefix $expectedAuditPrefix; found $($feature.audit_state)."
}

# The same no-secret version contract used by the MSI lifecycle gate must stay
# synchronized with the canonical product version. These checks run in the
# existing validate-powershell path before expensive build/package jobs.
$syntheticManifest = [pscustomobject]@{ product_version = $version }
$resolvedVersion = Resolve-V7CandidateVersion -RepositoryRoot $repo -CandidateManifest $syntheticManifest -BaselineVersion '7.0.0'
Assert-Version $version $resolvedVersion 'MSI lifecycle canonical candidate'
if (-not (Assert-V7CandidateMsiVersion -ExpectedVersion $resolvedVersion -MsiProductVersion $version)) {
    throw 'Matching MSI ProductVersion was rejected by the lifecycle version contract.'
}

$manifestMismatchRejected = $false
try {
    [void](Resolve-V7CandidateVersion -RepositoryRoot $repo -CandidateManifest ([pscustomobject]@{ product_version = '0.0.1' }) -BaselineVersion '7.0.0')
} catch {
    $manifestMismatchRejected = $true
}
if (-not $manifestMismatchRejected) {
    throw 'MSI lifecycle version contract did not reject a mismatched candidate manifest.'
}

$msiMismatchRejected = $false
try {
    [void](Assert-V7CandidateMsiVersion -ExpectedVersion $resolvedVersion -MsiProductVersion '0.0.1')
} catch {
    $msiMismatchRejected = $true
}
if (-not $msiMismatchRejected) {
    throw 'MSI lifecycle version contract did not reject a mismatched MSI ProductVersion.'
}

$nonUpgradeRejected = $false
try {
    [void](Resolve-V7CandidateVersion -RepositoryRoot $repo -CandidateManifest $syntheticManifest -BaselineVersion $version)
} catch {
    $nonUpgradeRejected = $true
}
if (-not $nonUpgradeRejected) {
    throw 'MSI lifecycle version contract did not reject a candidate that is not newer than its baseline.'
}

Write-Host "v7 version contract is synchronized at $version (release_ready=$([bool]$feature.release_ready), audit_state=$($feature.audit_state)); MSI lifecycle mismatch checks fail closed."
