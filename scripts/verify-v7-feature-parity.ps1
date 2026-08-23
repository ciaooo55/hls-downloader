[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
$path = Join-Path $repo 'docs\feature-parity.json'
$json = [IO.File]::ReadAllText($path, [Text.UTF8Encoding]::new($false)) | ConvertFrom-Json

if ($json.product_version -ne '7.0.0') { throw "Unexpected product version: $($json.product_version)" }
$features = @($json.features)
if ($features.Count -eq 0) { throw 'Feature parity matrix is empty.' }
$verified = @($features | Where-Object status -eq 'verified')
$requiredFields = @('id','legacy_entry','v7_entry','core_contract','state_events','tests','status')
foreach ($feature in $features) {
    foreach ($field in $requiredFields) {
        if ($null -eq $feature.$field -or @($feature.$field).Count -eq 0) {
            throw "Feature '$($feature.id)' is missing '$field'."
        }
    }
}
if (@($features.id | Sort-Object -Unique).Count -ne $features.Count) { throw 'Feature IDs must be unique.' }
if ($verified.Count -ne $features.Count) { throw "Unverified features: $($features.Count - $verified.Count)" }
if ($json.summary.total -ne $features.Count -or $json.summary.verified -ne $verified.Count -or $json.summary.percent -ne 100) {
    throw 'Feature parity summary does not match the feature records.'
}

Write-Output "FEATURE_PARITY=100% ($($verified.Count)/$($features.Count) verified)"
