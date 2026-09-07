[CmdletBinding()]
param(
    [string]$ManifestPath = 'artifacts\v7-productization\package\ARTIFACT-MANIFEST.json',
    [string]$CertificateThumbprint = $env:HLS_V7_SIGN_CERT_THUMBPRINT,
    [ValidateSet('CurrentUser', 'LocalMachine')][string]$CertificateStore = $(if ($env:HLS_V7_SIGN_CERT_STORE) { $env:HLS_V7_SIGN_CERT_STORE } else { 'CurrentUser' })
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$manifestFullPath = if ([IO.Path]::IsPathRooted($ManifestPath)) { [IO.Path]::GetFullPath($ManifestPath) } else { [IO.Path]::GetFullPath((Join-Path $repo $ManifestPath)) }
$manifest = Get-Content -LiteralPath $manifestFullPath -Raw -Encoding UTF8 | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
if ([int]$manifest.schema -ne 1 -or [string]$manifest.package_tier -ne 'formal' -or [string]$manifest.source_commit -ne $currentCommit -or [string]$manifest.source_tree -ne $currentTree) {
    throw 'Authenticode verification requires a formal package from the current source commit/tree.'
}
$root = Split-Path $manifestFullPath -Parent
function Resolve-And-Hash([string]$RelativePath, [string]$ExpectedHash) {
    $path = [IO.Path]::GetFullPath((Join-Path $root $RelativePath))
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Formal artifact is missing: $path" }
    $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $ExpectedHash.ToLowerInvariant()) { throw "Formal artifact SHA-256 mismatch: $RelativePath" }
    return $path
}
$exe = Resolve-And-Hash ([string]$manifest.artifacts.exe.path) ([string]$manifest.artifacts.exe.sha256)
$msi = Resolve-And-Hash ([string]$manifest.artifacts.msi.path) ([string]$manifest.artifacts.msi.sha256)
$portable = Resolve-And-Hash ([string]$manifest.artifacts.portable.path) ([string]$manifest.artifacts.portable.sha256)
& (Join-Path $PSScriptRoot 'sign-v7-authenticode.ps1') -VerifyOnly -Path @($exe, $msi) -CertificateThumbprint $CertificateThumbprint -CertificateStore $CertificateStore | Out-Null
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$stage = Join-Path $repo ('artifacts\v7-productization\.verify-signature-' + [guid]::NewGuid().ToString('n'))
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
    & (Join-Path $PSScriptRoot 'sign-v7-authenticode.ps1') -VerifyOnly -Path $firstParty -CertificateThumbprint $CertificateThumbprint -CertificateStore $CertificateStore | Out-Null
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
$report = Join-Path $root 'AUTHENTICODE-SIGNATURES.json'
if (-not (Test-Path -LiteralPath $report -PathType Leaf)) { throw 'Authenticode signature report is missing.' }
$reportJson = Get-Content -LiteralPath $report -Raw -Encoding UTF8 | ConvertFrom-Json
if ([int]$reportJson.schema -ne 1 -or [string]$reportJson.source_commit -ne $currentCommit -or [string]$reportJson.source_tree -ne $currentTree -or [string]$reportJson.signer_thumbprint -ne (($CertificateThumbprint -replace '\s', '').ToLowerInvariant())) {
    throw 'Authenticode signature report is not bound to this release source/signer.'
}
Write-Output ([ordered]@{ schema = 1; passed = $true; product_version = [string]$manifest.product_version; source_commit = $currentCommit; signer_thumbprint = [string]$reportJson.signer_thumbprint } | ConvertTo-Json -Compress)
