$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$root = Join-Path $repo ('.tool-cache\test-tmp\received-msi-contract-' + [guid]::NewGuid().ToString('N'))
$candidate = Join-Path $root 'candidate'
$evidence = Join-Path $root 'msi-lifecycle'
New-Item -ItemType Directory -Force -Path $candidate, $evidence | Out-Null
function Write-Json([string]$Path, $Value) { [IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false)) }
try {
    $msi = Join-Path $candidate 'fixture.msi'
    [IO.File]::WriteAllText($msi, 'evidence-contract-fixture')
    $manifestPath = Join-Path $candidate 'ARTIFACT-MANIFEST.json'
    $manifest = @{ source_commit = 'a' * 40; source_tree = 'b' * 40; product_version = '7.0.3'; artifacts = @{ msi = @{ path = 'fixture.msi'; sha256 = (Get-FileHash $msi -Algorithm SHA256).Hash.ToLowerInvariant() } } }
    Write-Json $manifestPath $manifest
    Write-Json (Join-Path $root 'RECEIPT.json') @{ source_commit = $manifest.source_commit; source_tree = $manifest.source_tree; run_url = 'fixture-only' }
    $names = @('install-old-exit', 'old-product-registered', 'real-task-checkpoint-created', 'uninstall-exit', 'product-unregistered', 'upgrade-exit', 'product-upgraded', 'task-restored-and-resumed', 'resumed-file-sha256', 'registration-present', 'shortcuts-present', 'application-process-restarted')
    $report = @{ schema = 1; scenario = 'Upgrade'; status = 'passed'; source_commit = $manifest.source_commit; source_tree = $manifest.source_tree; candidate_version = $manifest.product_version; candidate_manifest_sha256 = (Get-FileHash $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant(); candidate_msi_sha256 = $manifest.artifacts.msi.sha256; steps = @($names | ForEach-Object { @{ name = $_; passed = $true } }) }
    $reportPath = Join-Path $evidence 'upgrade.json'
    Write-Json $reportPath $report
    & (Join-Path $PSScriptRoot 'verify-v7-received-msi.ps1') -Scenario Upgrade -CandidateManifestPath $manifestPath -EvidenceRoot $evidence | Out-Null
    $rejected = 0
    foreach ($case in @('source', 'manifest', 'msi', 'missing-step', 'failed-step', 'failed-status')) {
        $bad = $report | ConvertTo-Json -Depth 8 | ConvertFrom-Json
        switch ($case) {
            'source' { $bad.source_commit = 'c' * 40 }
            'manifest' { $bad.candidate_manifest_sha256 = '0' * 64 }
            'msi' { $bad.candidate_msi_sha256 = '0' * 64 }
            'missing-step' { $bad.steps = @($bad.steps | Where-Object name -ne 'task-restored-and-resumed') }
            'failed-step' { $bad.steps[0].passed = $false }
            'failed-status' { $bad.status = 'failed' }
        }
        Write-Json $reportPath $bad
        $failed = $false
        try { & (Join-Path $PSScriptRoot 'verify-v7-received-msi.ps1') -Scenario Upgrade -CandidateManifestPath $manifestPath -EvidenceRoot $evidence | Out-Null } catch { $failed = $true }
        if (-not $failed) { throw "Tampered evidence was accepted: $case" }
        $rejected++
    }
    Write-Output "Received MSI evidence contract passed: valid fixture accepted, $rejected tampered reports rejected. No MSI was executed."
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force
}
