[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$CandidateManifestPath
)

$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'This real installer repair test runs only on an isolated GitHub Windows runner.'
}
. (Join-Path $PSScriptRoot 'V7MsiInspection.ps1')
. (Join-Path $PSScriptRoot 'V7MsiExecution.ps1')
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$evidence = Join-Path $repo 'artifacts\v7-productization\custom-pipe-maintenance'
$install = Join-Path $repo '.tool-cache\test-tmp\custom-pipe-maintenance\installed'
$manifestPath = (Resolve-Path -LiteralPath $CandidateManifestPath).Path
$manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding utf8 | ConvertFrom-Json
$msi = Join-Path (Split-Path $manifestPath) $manifest.artifacts.msi.path
if ((Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLowerInvariant() -ne $manifest.artifacts.msi.sha256) {
    throw 'Candidate MSI hash mismatch.'
}
$upgradeCode = Get-V7MsiProperty $msi 'UpgradeCode'
$productCode = Get-V7MsiProperty $msi 'ProductCode'
if ((Test-Path -LiteralPath $install) -or $null -ne (Get-V7InstalledProduct $upgradeCode)) {
    throw 'Repair validation requires a clean isolated installation.'
}
New-Item -ItemType Directory -Path $evidence -Force | Out-Null
$installed = $false
try {
    $exit = Invoke-V7Msi @('/i', $msi, '/qn', "INSTALLDIR=$install") (Join-Path $evidence 'install.log')
    if ($exit -notin @(0, 3010)) { throw "Candidate installation failed: $exit" }
    $installed = $true
    & python (Join-Path $PSScriptRoot 'custom_pipe_maintenance_acceptance.py') --msi $msi --engine (Join-Path $install 'app\resources\HLSDownloaderEngine.exe') --report-root $evidence
    if ($LASTEXITCODE -ne 0) { throw 'Custom-pipe repair or checkpoint regression failed.' }
} finally {
    if ($installed) {
        $exit = Invoke-V7Msi @('/x', $productCode, '/qn') (Join-Path $evidence 'uninstall.log')
        if ($exit -notin @(0, 3010)) { throw "Candidate cleanup failed: $exit" }
    }
}
