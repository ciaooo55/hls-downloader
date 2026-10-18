[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][ValidateSet('Upgrade', 'FailureRollback')][string]$Scenario,
    [Parameter(Mandatory=$true)][string]$CandidateManifestPath,
    [Parameter(Mandatory=$true)][string]$EvidenceRoot
)
$ErrorActionPreference = 'Stop'
$manifest = Get-Content -LiteralPath $CandidateManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
$receipt = Get-Content (Join-Path (Split-Path $EvidenceRoot) 'RECEIPT.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$reportPath = Join-Path $EvidenceRoot ($Scenario.ToLowerInvariant() + '.json')
$report = Get-Content -LiteralPath $reportPath -Raw -Encoding UTF8 | ConvertFrom-Json
$manifestHash = (Get-FileHash $CandidateManifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
$msi = Join-Path (Split-Path $CandidateManifestPath) ([string]$manifest.artifacts.msi.path)
if ($receipt.source_commit -ne $manifest.source_commit -or $receipt.source_tree -ne $manifest.source_tree -or
    $report.schema -ne 1 -or $report.status -ne 'passed' -or $report.scenario -ne $Scenario -or
    $report.source_commit -ne $manifest.source_commit -or $report.source_tree -ne $manifest.source_tree -or
    $report.candidate_version -ne $manifest.product_version -or $report.candidate_manifest_sha256 -ne $manifestHash -or
    $report.candidate_msi_sha256 -ne $manifest.artifacts.msi.sha256 -or
    (Get-FileHash $msi -Algorithm SHA256).Hash.ToLowerInvariant() -ne $report.candidate_msi_sha256) {
    throw 'Received MSI lifecycle evidence does not match the candidate and authenticated receipt.'
}
$required = @('install-old-exit', 'old-product-registered', 'real-task-checkpoint-created', 'uninstall-exit', 'product-unregistered')
$required += if ($Scenario -eq 'Upgrade') { @('upgrade-exit', 'product-upgraded', 'task-restored-and-resumed', 'resumed-file-sha256', 'registration-present', 'shortcuts-present', 'application-process-restarted') } else { @('forced-failure-exit', 'old-product-code-preserved', 'old-engine-preserved', 'database-bytes-preserved', 'task-and-data-preserved', 'registration-preserved', 'old-application-launches') }
foreach ($name in $required) {
    $step = @($report.steps | Where-Object name -eq $name)
    if ($step.Count -ne 1 -or $step[0].passed -ne $true) { throw "Missing or failed lifecycle assertion: $name" }
}
if (@($report.steps | Where-Object passed -ne $true).Count -ne 0) { throw 'A lifecycle assertion failed.' }
Write-Output ([ordered]@{ schema = 1; passed = $true; scenario = $Scenario; execution_location = 'GitHub-hosted isolated Windows'; run_url = $receipt.run_url; candidate_manifest_sha256 = $manifestHash; candidate_msi_sha256 = $report.candidate_msi_sha256; report = $report } | ConvertTo-Json -Depth 10 -Compress)
