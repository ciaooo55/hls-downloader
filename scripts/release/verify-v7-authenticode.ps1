[CmdletBinding()]
param(
    [string]$ManifestPath = 'artifacts\v7-productization\package\ARTIFACT-MANIFEST.json',
    [string]$CertificateThumbprint = $env:HLS_V7_SIGN_CERT_THUMBPRINT,
    [ValidateSet('CurrentUser', 'LocalMachine')][string]$CertificateStore = $(if ($env:HLS_V7_SIGN_CERT_STORE) { $env:HLS_V7_SIGN_CERT_STORE } else { 'CurrentUser' })
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\V7HashFunctions.ps1')
Import-Module (Join-Path $PSScriptRoot 'V7ReleaseScope.psm1') -Force
Add-Type -AssemblyName System.IO.Compression.FileSystem
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$manifestFullPath = if ([IO.Path]::IsPathRooted($ManifestPath)) { [IO.Path]::GetFullPath($ManifestPath) } else { [IO.Path]::GetFullPath((Join-Path $repo $ManifestPath)) }
$manifest = Get-Content -LiteralPath $manifestFullPath -Raw -Encoding UTF8 | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
if ([int]$manifest.schema -ne 1 -or [string]$manifest.package_tier -ne 'formal' -or [string]$manifest.source_commit -ne $currentCommit -or [string]$manifest.source_tree -ne $currentTree) {
    throw 'Authenticode verification requires a formal package from the current source commit/tree.'
}
$root = Split-Path $manifestFullPath -Parent
$exe = Resolve-V7PackageFile $root $manifest.artifacts.exe.path $manifest.artifacts.exe.sha256
$msi = Resolve-V7PackageFile $root $manifest.artifacts.msi.path $manifest.artifacts.msi.sha256
$portable = Resolve-V7PackageFile $root $manifest.artifacts.portable.path $manifest.artifacts.portable.sha256
$records = @(& (Join-Path $PSScriptRoot 'sign-v7-authenticode.ps1') -VerifyOnly -Path @($exe, $msi) -CertificateThumbprint $CertificateThumbprint -CertificateStore $CertificateStore | ForEach-Object { $_ | ConvertFrom-Json })
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$stage = Join-Path $repo ('artifacts\v7-productization\.verify-signature-' + [guid]::NewGuid().ToString('n'))
try {
    [IO.Compression.ZipFile]::ExtractToDirectory($portable, $stage)
    $portableRoot = Join-Path $stage 'HLSDownloader'
    $firstParty = @(foreach ($name in Get-V7PortableExecutables) {
        Resolve-V7PackageFile $portableRoot $name $manifest.components.$name
    })
    $records += @(& (Join-Path $PSScriptRoot 'sign-v7-authenticode.ps1') -VerifyOnly -Path $firstParty -CertificateThumbprint $CertificateThumbprint -CertificateStore $CertificateStore | ForEach-Object { $_ | ConvertFrom-Json })
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
if ($reportJson.product_version -ne $manifest.product_version) { throw 'Signature report version mismatch.' }
foreach ($name in @('exe', 'msi', 'portable')) {
    if ($reportJson.artifacts.$name.path -cne $manifest.artifacts.$name.path -or
        $reportJson.artifacts.$name.sha256 -ne $manifest.artifacts.$name.sha256) {
        throw "Signature report artifact mismatch: $name"
    }
}
$paths = @($manifest.artifacts.exe.path, $manifest.artifacts.msi.path)
$paths += @(Get-V7PortableExecutables | ForEach-Object { "HLSDownloader/$_" })
if (@($reportJson.verified_signatures).Count -ne $paths.Count -or $records.Count -ne $paths.Count) {
    throw 'Signature report has missing or unexpected entries.'
}
for ($i = 0; $i -lt $paths.Count; $i++) {
    $row = @($reportJson.verified_signatures | Where-Object { $_.path -ceq $paths[$i] })
    if ($row.Count -ne 1 -or $row[0].status -ne $records[$i].status -or
        $row[0].signer_thumbprint -ne $records[$i].signer_thumbprint -or
        $row[0].timestamp_thumbprint -ne $records[$i].timestamp_thumbprint) {
        throw "Signature report differs from the verified signature: $($paths[$i])"
    }
}
Write-Output ([ordered]@{ schema = 1; passed = $true; product_version = [string]$manifest.product_version; source_commit = $currentCommit; signer_thumbprint = [string]$reportJson.signer_thumbprint } | ConvertTo-Json -Compress)
