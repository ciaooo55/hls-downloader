Set-StrictMode -Version Latest

function Resolve-V7CandidateVersion {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string]$RepositoryRoot,

        [Parameter(Mandatory = $true)]
        $CandidateManifest,

        [string]$BaselineVersion = '7.0.0'
    )

    $featureParityPath = Join-Path $RepositoryRoot 'artifacts\v7-productization\feature-parity.json'
    if (-not (Test-Path -LiteralPath $featureParityPath -PathType Leaf)) {
        throw "Canonical feature parity matrix is missing: $featureParityPath"
    }

    $utf8NoBom = New-Object Text.UTF8Encoding($false)
    $featureParity = [IO.File]::ReadAllText($featureParityPath, $utf8NoBom) | ConvertFrom-Json
    if ([int]$featureParity.schema -ne 1) {
        throw "Unsupported feature parity schema: $($featureParity.schema)"
    }

    $expectedVersionText = [string]$featureParity.product_version
    if ([String]::IsNullOrWhiteSpace($expectedVersionText)) {
        throw 'Canonical feature parity product_version is missing.'
    }

    try {
        $expectedVersion = [version]$expectedVersionText
    } catch {
        throw "Canonical feature parity product_version is not a valid version: $expectedVersionText"
    }

    try {
        $baseline = [version]$BaselineVersion
    } catch {
        throw "Baseline MSI version is not a valid version: $BaselineVersion"
    }

    if ($expectedVersion -le $baseline) {
        throw "Candidate version must be newer than the public baseline: $expectedVersionText <= $BaselineVersion"
    }

    $manifestVersion = [string]$CandidateManifest.product_version
    if ($manifestVersion -ne $expectedVersionText) {
        throw "Candidate manifest product_version does not match canonical product version: $manifestVersion != $expectedVersionText"
    }

    return $expectedVersionText
}

function Assert-V7CandidateMsiVersion {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string]$ExpectedVersion,

        [Parameter(Mandatory = $true)]
        [string]$MsiProductVersion
    )

    if ([String]::IsNullOrWhiteSpace($MsiProductVersion)) {
        throw 'Candidate MSI ProductVersion is missing.'
    }

    try {
        [void][version]$ExpectedVersion
        [void][version]$MsiProductVersion
    } catch {
        throw "Candidate version contract contains an invalid version: expected=$ExpectedVersion msi=$MsiProductVersion"
    }

    if ($MsiProductVersion -ne $ExpectedVersion) {
        throw "Candidate MSI ProductVersion does not match canonical product version: $MsiProductVersion != $ExpectedVersion"
    }

    return $true
}

Export-ModuleMember -Function Resolve-V7CandidateVersion, Assert-V7CandidateMsiVersion
