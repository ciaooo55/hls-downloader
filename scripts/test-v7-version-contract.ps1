[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Import-Module (Join-Path $PSScriptRoot 'V7VersionContract.psm1') -Force

$featureParityPath = Join-Path $repo 'artifacts\v7-productization\feature-parity.json'
$featureParity = Get-Content -LiteralPath $featureParityPath -Raw | ConvertFrom-Json
$currentVersion = [string]$featureParity.product_version
if ([String]::IsNullOrWhiteSpace($currentVersion)) {
    throw 'Canonical feature parity product_version is missing.'
}

$goodManifest = [pscustomobject]@{ product_version = $currentVersion }
$resolved = Resolve-V7CandidateVersion -RepositoryRoot $repo -CandidateManifest $goodManifest -BaselineVersion '7.0.0'
if ($resolved -ne $currentVersion) {
    throw "Resolved candidate version drifted from canonical product version: $resolved != $currentVersion"
}

if (-not (Assert-V7CandidateMsiVersion -ExpectedVersion $resolved -MsiProductVersion $currentVersion)) {
    throw 'Matching candidate MSI ProductVersion was rejected.'
}

$manifestMismatchRejected = $false
try {
    [void](Resolve-V7CandidateVersion -RepositoryRoot $repo -CandidateManifest ([pscustomobject]@{ product_version = '0.0.1' }) -BaselineVersion '7.0.0')
} catch {
    $manifestMismatchRejected = $true
}
if (-not $manifestMismatchRejected) {
    throw 'Mismatched candidate manifest version was not rejected.'
}

$msiMismatchRejected = $false
try {
    [void](Assert-V7CandidateMsiVersion -ExpectedVersion $resolved -MsiProductVersion '0.0.1')
} catch {
    $msiMismatchRejected = $true
}
if (-not $msiMismatchRejected) {
    throw 'Mismatched candidate MSI ProductVersion was not rejected.'
}

$nonUpgradeRejected = $false
try {
    [void](Resolve-V7CandidateVersion -RepositoryRoot $repo -CandidateManifest $goodManifest -BaselineVersion $currentVersion)
} catch {
    $nonUpgradeRejected = $true
}
if (-not $nonUpgradeRejected) {
    throw 'Candidate version equal to the baseline was not rejected.'
}

Write-Host "v7 version contract PASS: canonical=$currentVersion baseline=7.0.0 manifest/msi mismatch checks fail closed."
