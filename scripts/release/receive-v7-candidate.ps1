#requires -Version 5.1
[CmdletBinding()]
param([ValidatePattern('\A[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\z')][string]$Repository = 'ciaooo55/hls-downloader')

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'V7ReleaseScope.psm1') -Force
. (Join-Path $PSScriptRoot '..\V7HashFunctions.ps1')
Add-Type -AssemblyName System.IO.Compression.FileSystem
if ([String]::IsNullOrWhiteSpace($env:GH_TOKEN)) { throw 'An Actions-read GH_TOKEN is required to receive authenticated artifacts.' }
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$sha = (& git -C $repo rev-parse HEAD).Trim()
$tree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
$runs = & gh.exe api "repos/$Repository/actions/workflows/package-v7-candidate.yml/runs?head_sha=$sha&status=success&per_page=100" | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Could not query authenticated candidate runs.' }
$run = @($runs.workflow_runs | Where-Object { $_.head_sha -eq $sha -and $_.head_branch -eq 'main' -and $_.event -eq 'workflow_dispatch' -and $_.conclusion -eq 'success' } | Sort-Object run_number -Descending | Select-Object -First 1)
if ($run.Count -ne 1) { throw "No successful candidate and MSI lifecycle run exists for main $sha." }
$root = Join-Path $repo ('artifacts\v7-productization\received-candidate-' + [long]$run[0].id + '-' + [guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($root)
try {
    $artifacts = & gh.exe api "repos/$Repository/actions/runs/$($run[0].id)/artifacts?per_page=100" | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Could not query candidate artifact metadata.' }
    $entries = @(
        @{ Name = "HLSDownloader-v7-candidate-$sha"; Directory = 'candidate'; Available = $artifacts.artifacts },
        @{ Name = "HLSDownloader-v7-msi-lifecycle-$sha"; Directory = 'msi-lifecycle'; Available = $artifacts.artifacts }
    )
    $regressionRuns = @{}
    foreach ($check in @(
        @{ Workflow = 'verify-v7-candidate-browsers.yml'; Artifact = "HLSDownloader-browser-regression-$sha"; Directory = 'browser-regression' },
        @{ Workflow = 'verify-v7-native-candidate.yml'; Artifact = "HLSDownloader-v7-native-acceptance-$sha"; Directory = 'native-regression' }
    )) {
        $checks = & gh.exe api "repos/$Repository/actions/workflows/$($check.Workflow)/runs?head_sha=$sha&status=success&per_page=100" | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { throw "Could not query $($check.Workflow)." }
        $checkRun = @($checks.workflow_runs | Where-Object { $_.head_sha -eq $sha -and $_.head_branch -eq 'main' -and $_.event -eq 'workflow_dispatch' -and $_.conclusion -eq 'success' } | Sort-Object run_number -Descending | Select-Object -First 1)
        if ($checkRun.Count -ne 1) { throw "No successful same-source $($check.Workflow) exists for $sha." }
        $metadata = & gh.exe api "repos/$Repository/actions/runs/$($checkRun[0].id)/artifacts?per_page=100" | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0) { throw 'Could not query regression artifacts.' }
        $entries += @{ Name = $check.Artifact; Directory = $check.Directory; Available = $metadata.artifacts }
        $regressionRuns[$check.Directory] = [ordered]@{ run_id = $checkRun[0].id; url = $checkRun[0].html_url; source_commit = $sha }
    }
    foreach ($entry in $entries) {
        $destination = Join-Path $root $entry.Directory
        if (Test-Path -LiteralPath $destination) { throw "Refusing to overwrite received evidence: $destination" }
        $artifact = @($entry.Available | Where-Object { $_.name -eq $entry.Name -and -not $_.expired })
        if ($artifact.Count -ne 1 -or $artifact[0].digest -notmatch '^sha256:[0-9a-f]{64}$') { throw "Missing authenticated artifact identity: $($entry.Name)" }
        # 此机器上 gh 的大文件重定向下载会停在零字节；curl 强制 IPv4，并限定连接和总耗时。
        $request = [Net.HttpWebRequest]::Create("https://api.github.com/repos/$Repository/actions/artifacts/$($artifact[0].id)/zip")
        $request.AllowAutoRedirect = $false
        $request.UserAgent = 'hls-release-receiver'
        $request.Accept = 'application/vnd.github+json'
        $request.Headers['Authorization'] = "Bearer $env:GH_TOKEN"
        $request.Timeout = 60000
        $response = $request.GetResponse()
        try {
            $downloadUrl = [uri]$response.Headers['Location']
            if ([int]$response.StatusCode -ne 302 -or -not $downloadUrl.IsAbsoluteUri -or $downloadUrl.Scheme -ne 'https') {
                throw 'GitHub did not return an HTTPS artifact redirect.'
            }
        } finally {
            $response.Dispose()
        }
        $archive = Join-Path $root ($entry.Directory + '.zip')
        & curl.exe --ipv4 --location --proto =https --proto-redir =https --fail --silent --show-error --connect-timeout 15 --max-time 1800 --speed-limit 1024 --speed-time 60 --output $archive $downloadUrl.AbsoluteUri
        if ($LASTEXITCODE -ne 0) { throw "Authenticated artifact download failed: $($entry.Name)" }
        $expectedDigest = ([string]$artifact[0].digest).Substring(7)
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedDigest) { throw 'Downloaded artifact ZIP digest differs from the authenticated GitHub digest.' }
        [IO.Compression.ZipFile]::ExtractToDirectory($archive, $destination)
        Remove-Item -LiteralPath $archive
    }
    $manifestPath = Join-Path $root 'candidate\ARTIFACT-MANIFEST.json'
    $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($manifest.schema -ne 1 -or $manifest.source_commit -ne $sha -or $manifest.source_tree -ne $tree -or $manifest.package_tier -ne 'candidate') { throw 'Received candidate has another source identity.' }
    $candidateRoot = Split-Path $manifestPath
    foreach ($name in @('exe', 'msi', 'portable')) {
        $entry = $manifest.artifacts.$name
        [void](Resolve-V7PackageFile $candidateRoot $entry.path $entry.sha256)
    }
    foreach ($name in @('Chromium', 'Firefox')) {
        $entry = $manifest.extensions.$name
        [void](Resolve-V7PackageFile $candidateRoot $entry.path $entry.sha256)
    }
    [void](Resolve-V7PackageFile $candidateRoot $manifest.feature_parity_path $manifest.feature_parity_sha256)
    $provenance = [ordered]@{ schema = 1; repository = $Repository; run_id = $run[0].id; run_url = $run[0].html_url; source_commit = $sha; source_tree = $tree; candidate_manifest = $manifestPath; msi_evidence = (Join-Path $root 'msi-lifecycle'); browser_evidence = (Join-Path $root 'browser-regression'); native_evidence = (Join-Path $root 'native-regression'); regression_runs = $regressionRuns }
    [IO.File]::WriteAllText((Join-Path $root 'RECEIPT.json'), ($provenance | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
    foreach ($scenario in @('Upgrade', 'FailureRollback')) {
        & (Join-Path $PSScriptRoot '..\validation\verify-v7-received-msi.ps1') -Scenario $scenario -CandidateManifestPath $manifestPath -EvidenceRoot $provenance.msi_evidence | Out-Null
    }
    foreach ($kind in @('browser', 'native')) {
        & python.exe (Join-Path $PSScriptRoot '..\validation\verify_v7_received_regression.py') --kind $kind --manifest $manifestPath --evidence (Join-Path $root "$kind-regression") --candidate-run-id $run[0].id
        if ($LASTEXITCODE -ne 0) { throw "Received $kind regression failed identity or result validation." }
    }
} catch {
    Remove-Item -LiteralPath $root -Recurse -Force
    throw
}
Write-Output ($provenance | ConvertTo-Json -Depth 6 -Compress)
