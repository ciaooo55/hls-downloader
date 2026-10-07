#requires -Version 7.0
[CmdletBinding()]
param([string]$Repository = 'ciaooo55/hls-downloader')

$ErrorActionPreference = 'Stop'
if ([String]::IsNullOrWhiteSpace($env:GH_TOKEN)) { throw 'An Actions-read GH_TOKEN is required to receive authenticated artifacts.' }
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$sha = (& git -C $repo rev-parse HEAD).Trim()
$tree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
$runs = & gh.exe api "repos/$Repository/actions/workflows/package-v7-candidate.yml/runs?head_sha=$sha&status=success&per_page=100" | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Could not query authenticated candidate runs.' }
$run = @($runs.workflow_runs | Where-Object { $_.head_sha -eq $sha -and $_.head_branch -eq 'main' -and $_.event -eq 'workflow_dispatch' -and $_.conclusion -eq 'success' } | Sort-Object run_number -Descending | Select-Object -First 1)
if ($run.Count -ne 1) { throw "No successful candidate and MSI lifecycle run exists for main $sha." }
$root = Join-Path $repo ('artifacts\v7-productization\received-candidate-' + $run[0].id + '-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $root | Out-Null
$artifacts = & gh.exe api "repos/$Repository/actions/runs/$($run[0].id)/artifacts?per_page=100" | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Could not query candidate artifact metadata.' }
$headers = @{ Authorization = "Bearer $env:GH_TOKEN"; Accept = 'application/vnd.github+json'; 'User-Agent' = 'hls-release-receiver' }
foreach ($entry in @(
    @{ Name = "HLSDownloader-v7-candidate-$sha"; Directory = 'candidate' },
    @{ Name = "HLSDownloader-v7-msi-lifecycle-$sha"; Directory = 'msi-lifecycle' }
)) {
    $destination = Join-Path $root $entry.Directory
    if (Test-Path -LiteralPath $destination) { throw "Refusing to overwrite received evidence: $destination" }
    $artifact = @($artifacts.artifacts | Where-Object { $_.name -eq $entry.Name -and -not $_.expired })
    if ($artifact.Count -ne 1 -or $artifact[0].digest -notmatch '^sha256:[0-9a-f]{64}$') { throw "Missing authenticated artifact identity: $($entry.Name)" }
    # 此机器上 gh 的大文件重定向下载会停在零字节；curl 强制 IPv4，并限定连接和总耗时。
    $response = Invoke-WebRequest -Uri "https://api.github.com/repos/$Repository/actions/artifacts/$($artifact[0].id)/zip" -Headers $headers -TimeoutSec 60 -MaximumRedirection 0 -SkipHttpErrorCheck -ErrorAction SilentlyContinue
    if (-not $response.Headers.Location) { throw 'Artifact redirect was not returned.' }
    $downloadUrl = [string]$response.Headers.Location[0]
    $archive = Join-Path $root ($entry.Directory + '.zip')
    & curl.exe --ipv4 --location --fail --silent --show-error --connect-timeout 15 --max-time 1800 --speed-limit 1024 --speed-time 60 --output $archive $downloadUrl
    if ($LASTEXITCODE -ne 0) { throw "Authenticated artifact download failed: $($entry.Name)" }
    $expectedDigest = ([string]$artifact[0].digest).Substring(7)
    if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expectedDigest) { throw 'Downloaded artifact ZIP digest differs from the authenticated GitHub digest.' }
    Expand-Archive -LiteralPath $archive -DestinationPath $destination
    Remove-Item -LiteralPath $archive
}
$manifestPath = Join-Path $root 'candidate\ARTIFACT-MANIFEST.json'
$manifest = Get-Content $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ($manifest.source_commit -ne $sha -or $manifest.source_tree -ne $tree -or $manifest.package_tier -ne 'candidate') { throw 'Received candidate has another source identity.' }
$candidateRoot = Split-Path $manifestPath
foreach ($entry in $manifest.artifacts.PSObject.Properties) {
    $path = [IO.Path]::GetFullPath((Join-Path $candidateRoot ([string]$entry.Value.path)))
    if (-not $path.StartsWith($candidateRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Candidate artifact escapes its directory.' }
    if ((Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entry.Value.sha256) { throw "Candidate digest mismatch: $($entry.Name)" }
}
$provenance = [ordered]@{ schema = 1; repository = $Repository; run_id = $run[0].id; run_url = $run[0].html_url; source_commit = $sha; source_tree = $tree; candidate_manifest = $manifestPath; msi_evidence = (Join-Path $root 'msi-lifecycle') }
[IO.File]::WriteAllText((Join-Path $root 'RECEIPT.json'), ($provenance | ConvertTo-Json), [Text.UTF8Encoding]::new($false))
foreach ($scenario in @('Upgrade', 'FailureRollback')) {
    & (Join-Path $PSScriptRoot 'verify-v7-received-msi.ps1') -Scenario $scenario -CandidateManifestPath $manifestPath -EvidenceRoot $provenance.msi_evidence | Out-Null
}
Write-Output ($provenance | ConvertTo-Json -Compress)
