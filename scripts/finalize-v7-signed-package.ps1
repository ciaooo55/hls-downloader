[CmdletBinding()]
param(
    [string]$ManifestPath = 'artifacts\v7-productization\package\ARTIFACT-MANIFEST.json',
    [string]$CertificateThumbprint = $env:HLS_V7_SIGN_CERT_THUMBPRINT,
    [ValidateSet('CurrentUser', 'LocalMachine')][string]$CertificateStore = $(if ($env:HLS_V7_SIGN_CERT_STORE) { $env:HLS_V7_SIGN_CERT_STORE } else { 'CurrentUser' }),
    [string]$TimestampUrl = $(if ($env:HLS_V7_TIMESTAMP_URL) { $env:HLS_V7_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' })
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$repoPrefix = $repo.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
$manifestFullPath = if ([IO.Path]::IsPathRooted($ManifestPath)) { [IO.Path]::GetFullPath($ManifestPath) } else { [IO.Path]::GetFullPath((Join-Path $repo $ManifestPath)) }
if (-not $manifestFullPath.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'Formal package manifest must stay inside the repository.' }
if (-not (Test-Path -LiteralPath $manifestFullPath -PathType Leaf)) { throw "Formal artifact manifest is missing: $manifestFullPath" }
$manifest = Get-Content -LiteralPath $manifestFullPath -Raw -Encoding UTF8 | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
if ([int]$manifest.schema -ne 1 -or [string]$manifest.package_tier -ne 'formal' -or [string]$manifest.source_commit -ne $currentCommit -or [string]$manifest.source_tree -ne $currentTree) {
    throw 'Formal package manifest is not bound to the current source commit/tree.'
}
$root = Split-Path $manifestFullPath -Parent
function Resolve-Artifact([string]$RelativePath) {
    $full = [IO.Path]::GetFullPath((Join-Path $root $RelativePath))
    if (-not $full.StartsWith(($root.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) { throw "Artifact escapes formal package root: $RelativePath" }
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Formal artifact is missing: $full" }
    return $full
}
function Invoke-Signer([string[]]$Targets) {
    $output = @(& (Join-Path $PSScriptRoot 'sign-v7-authenticode.ps1') -Path $Targets -CertificateThumbprint $CertificateThumbprint -CertificateStore $CertificateStore -TimestampUrl $TimestampUrl)
    if ($LASTEXITCODE -ne 0) { throw "Authenticode signer failed with exit code $LASTEXITCODE." }
    return @($output | Where-Object { -not [String]::IsNullOrWhiteSpace($_) } | ForEach-Object { $_ | ConvertFrom-Json })
}

$exe = Resolve-Artifact ([string]$manifest.artifacts.exe.path)
$msi = Resolve-Artifact ([string]$manifest.artifacts.msi.path)
$portable = Resolve-Artifact ([string]$manifest.artifacts.portable.path)
$signatureRows = New-Object Collections.ArrayList
foreach ($row in @(Invoke-Signer @($exe, $msi))) { [void]$signatureRows.Add($row) }

$stage = Join-Path $repo ('artifacts\v7-productization\.sign-portable-' + [guid]::NewGuid().ToString('n'))
try {
    Expand-Archive -LiteralPath $portable -DestinationPath $stage -Force
    $portableRoot = Join-Path $stage 'HLSDownloader'
    $firstParty = @(
        (Join-Path $portableRoot 'HLSDownloader.exe'),
        (Join-Path $portableRoot 'app\resources\HLSDownloaderEngine.exe'),
        (Join-Path $portableRoot 'app\resources\HLSDownloaderNativeHost.exe'),
        (Join-Path $portableRoot 'app\resources\HLSDownloaderUpdater.exe'),
        (Join-Path $portableRoot 'app\resources\HLSDownloaderPresenter.exe')
    )
    foreach ($target in $firstParty) {
        if (-not (Test-Path -LiteralPath $target -PathType Leaf)) { throw "Portable first-party binary is missing: $target" }
    }
    foreach ($row in @(Invoke-Signer $firstParty)) { [void]$signatureRows.Add($row) }
    Remove-Item -LiteralPath $portable -Force
    Compress-Archive -Path $portableRoot -DestinationPath $portable -CompressionLevel Optimal -Force
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}

$manifest.artifacts.exe.sha256 = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
$manifest.artifacts.msi.sha256 = (Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLowerInvariant()
$manifest.artifacts.portable.sha256 = (Get-FileHash -LiteralPath $portable -Algorithm SHA256).Hash.ToLowerInvariant()
$utf8NoBom = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText($manifestFullPath, ($manifest | ConvertTo-Json -Depth 10), $utf8NoBom)

$signatureReport = [ordered]@{
    schema = 1
    product_version = [string]$manifest.product_version
    source_commit = $currentCommit
    source_tree = $currentTree
    signer_thumbprint = (($CertificateThumbprint -replace '\s', '').ToLowerInvariant())
    certificate_store = $CertificateStore
    timestamp_url = $TimestampUrl
    signed_at_utc = [DateTime]::UtcNow.ToString('o')
    artifacts = [ordered]@{
        exe = [ordered]@{ path = [string]$manifest.artifacts.exe.path; sha256 = [string]$manifest.artifacts.exe.sha256 }
        msi = [ordered]@{ path = [string]$manifest.artifacts.msi.path; sha256 = [string]$manifest.artifacts.msi.sha256 }
        portable = [ordered]@{ path = [string]$manifest.artifacts.portable.path; sha256 = [string]$manifest.artifacts.portable.sha256 }
    }
    verified_signatures = @($signatureRows)
}
$reportPath = Join-Path $root 'AUTHENTICODE-SIGNATURES.json'
[IO.File]::WriteAllText($reportPath, ($signatureReport | ConvertTo-Json -Depth 8), $utf8NoBom)
Write-Output "SIGNED_FORMAL_PACKAGE=$root"
Write-Output "SIGNATURE_REPORT=$reportPath"
