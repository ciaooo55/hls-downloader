[CmdletBinding()]
param(
    [string]$FeatureParityPath = 'artifacts\v7-productization\feature-parity.json'
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$path = if ([IO.Path]::IsPathRooted($FeatureParityPath)) {
    [IO.Path]::GetFullPath($FeatureParityPath)
} else {
    [IO.Path]::GetFullPath((Join-Path $repo $FeatureParityPath))
}

if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Feature parity matrix is missing: $path"
}

try {
    $feature = Get-Content -LiteralPath $path -Raw -Encoding UTF8 | ConvertFrom-Json
} catch {
    throw "Feature parity matrix is not valid JSON: $($_.Exception.Message)"
}

$gaps = @(
    @($feature.features) |
        Where-Object {
            $_.PSObject.Properties.Name -contains 'gap' -and
            -not [String]::IsNullOrWhiteSpace([string]$_.gap)
        } |
        ForEach-Object {
            [pscustomobject]@{
                id = [string]$_.id
                status = [string]$_.status
                gap = [string]$_.gap
            }
        }
)

if ($gaps.Count -ne 0) {
    $summary = @($gaps | ForEach-Object { "$($_.id)[$($_.status)]: $($_.gap)" }) -join '; '
    throw "Formal release is blocked by residual canonical feature gap(s): $summary"
}

Write-Output ([ordered]@{
    schema = 1
    passed = $true
    feature_count = @($feature.features).Count
    residual_gap_count = 0
} | ConvertTo-Json -Compress)
