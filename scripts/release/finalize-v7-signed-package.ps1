[CmdletBinding()]
param(
    [string]$ManifestPath = 'artifacts\v7-productization\package\ARTIFACT-MANIFEST.json',
    [string]$CertificateThumbprint = $env:HLS_V7_SIGN_CERT_THUMBPRINT,
    [ValidateSet('CurrentUser', 'LocalMachine')][string]$CertificateStore = $(if ($env:HLS_V7_SIGN_CERT_STORE) { $env:HLS_V7_SIGN_CERT_STORE } else { 'CurrentUser' }),
    [string]$TimestampUrl = $(if ($env:HLS_V7_TIMESTAMP_URL) { $env:HLS_V7_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }),
    [string]$WixRoot = $(if ($env:HLS_V7_WIX_ROOT) { $env:HLS_V7_WIX_ROOT } else { '' }),
    [string]$SignToolPath = $env:HLS_V7_SIGNTOOL
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\V7HashFunctions.ps1')
. (Join-Path $PSScriptRoot '..\build\V7MsiIdentity.ps1')
Import-Module (Join-Path $PSScriptRoot 'V7ReleaseScope.psm1') -Force
Add-Type -AssemblyName System.IO.Compression.FileSystem
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
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
$cacheRoot = if ($env:HLS_V7_BUILD_CACHE) { [IO.Path]::GetFullPath($env:HLS_V7_BUILD_CACHE) } else { Join-Path $repo '.tool-cache\build-cache' }
$resolvedWixRoot = if (-not [String]::IsNullOrWhiteSpace($WixRoot)) {
    [IO.Path]::GetFullPath($WixRoot)
} else {
    Join-Path $cacheRoot 'wix-3.14.1'
}
foreach ($wixTool in @('candle.exe', 'light.exe', 'insignia.exe')) {
    $toolPath = Join-Path $resolvedWixRoot $wixTool
    if (-not (Test-Path -LiteralPath $toolPath -PathType Leaf)) {
        throw "Signed formal installer rebuild requires WiX tool $wixTool at $toolPath (set -WixRoot or HLS_V7_WIX_ROOT)."
    }
}
$root = Split-Path $manifestFullPath -Parent
function Invoke-Signer([string[]]$Targets, [switch]$VerifyOnly) {
    $signArgs = @{
        Path = $Targets
        CertificateThumbprint = $CertificateThumbprint
        CertificateStore = $CertificateStore
        TimestampUrl = $TimestampUrl
    }
    if (-not [String]::IsNullOrWhiteSpace($SignToolPath)) {
        $signArgs['SignToolPath'] = $SignToolPath
    }
    if ($VerifyOnly) {
        $signArgs['VerifyOnly'] = $true
    }
    $output = @(& (Join-Path $PSScriptRoot 'sign-v7-authenticode.ps1') @signArgs)
    if ($LASTEXITCODE -ne 0) { throw "Authenticode signer failed with exit code $LASTEXITCODE." }
    return @($output | Where-Object { -not [String]::IsNullOrWhiteSpace($_) } | ForEach-Object { $_ | ConvertFrom-Json })
}

$stage = Join-Path $root ('.sign-' + [guid]::NewGuid().ToString('n'))
[void][IO.Directory]::CreateDirectory($stage)
$signatureRows = New-Object Collections.ArrayList
$updates = [ordered]@{}
$applied = @()
$keepBackups = $false
try {
    foreach ($name in @('exe', 'msi', 'portable')) {
        $entry = $manifest.artifacts.$name
        $source = Resolve-V7PackageFile $root $entry.path $entry.sha256
        $extension = @{ exe = '.exe'; msi = '.msi'; portable = '.zip' }[$name]
        if ([IO.Path]::GetExtension($source) -ne $extension) { throw "Wrong artifact type: $source" }
        $copy = Join-Path $stage ($name + $extension)
        [IO.File]::Copy($source, $copy)
        if ((Get-FileHash -LiteralPath $copy -Algorithm SHA256).Hash -ne $entry.sha256) { throw "Artifact changed: $source" }
        $updates.Add($source, $copy)
    }
    $portable = Join-Path $stage 'portable.zip'
    $unpacked = Join-Path $stage 'unpacked'
    [IO.Compression.ZipFile]::ExtractToDirectory($portable, $unpacked)
    $portableRoot = Join-Path $unpacked 'HLSDownloader'
    $firstParty = @(foreach ($name in Get-V7PortableExecutables) {
        Resolve-V7PackageFile $portableRoot $name $manifest.components.$name
    })
    $exe = Join-Path $stage 'exe.exe'
    $msi = Join-Path $stage 'msi.msi'
    $recordPaths = @{}
    $recordPaths[$exe] = $manifest.artifacts.exe.path
    $recordPaths[$msi] = $manifest.artifacts.msi.path
    foreach ($name in Get-V7PortableExecutables) {
        $recordPaths[[IO.Path]::GetFullPath((Join-Path $portableRoot $name))] = "HLSDownloader/$name"
    }
    # 1. Sign every first-party EXE first (including scripts/HLSDownloaderPreflight.exe)
    #    so MSI, Burn bundle, and Portable all embed the exact same signed binaries.
    $signedPortableRows = @(Invoke-Signer $firstParty)

    # 2. Assemble the signed MSI app-image from the signed Portable tree (excluding portable-only files).
    $signedAppImage = Join-Path $stage 'signed-app-image\HLSDownloader'
    [void][IO.Directory]::CreateDirectory($signedAppImage)
    $portableOnlyTopLevel = @('portable', 'README-PORTABLE.txt', 'scripts', 'extensions')
    foreach ($item in Get-ChildItem -LiteralPath $portableRoot -Force) {
        if ($portableOnlyTopLevel -contains $item.Name) { continue }
        Copy-Item -LiteralPath $item.FullName -Destination (Join-Path $signedAppImage $item.Name) -Recurse -Force
    }

    # 3. Rebuild the MSI and Burn bundle from the signed EXEs and signed preflight helper:
    #    build-native-v7-installer.ps1 signs the MSI before Burn embeds it, signs the Burn engine
    #    via WiX insignia, and signs the outer Burn executable.
    [IO.File]::Delete($exe)
    [IO.File]::Delete($msi)
    $signedProductCode = New-V7MsiProductCode -SourceCommit $currentCommit -PackageTier 'formal-signed' -ProductVersion ([string]$manifest.product_version)
    $signedPreflight = Join-Path $portableRoot 'scripts\HLSDownloaderPreflight.exe'
    $installerArgs = @{
        AppImage = $signedAppImage
        WixRoot = $resolvedWixRoot
        OutMsi = $msi
        OutExe = $exe
        ProductVersion = [string]$manifest.product_version
        ProductCode = $signedProductCode
        PreflightPath = $signedPreflight
        CertificateThumbprint = $CertificateThumbprint
        CertificateStore = $CertificateStore
        TimestampUrl = $TimestampUrl
    }
    if (-not [String]::IsNullOrWhiteSpace($SignToolPath)) {
        $installerArgs['SignToolPath'] = $SignToolPath
    }
    & (Join-Path $PSScriptRoot '..\build\build-native-v7-installer.ps1') @installerArgs
    if ($LASTEXITCODE -ne 0) { throw "Signed installer rebuild failed with exit code $LASTEXITCODE." }

    $verifiedInstallerRows = @(Invoke-Signer @($exe, $msi) -VerifyOnly)
    foreach ($row in @($verifiedInstallerRows + $signedPortableRows)) {
        $row.path = $recordPaths[$row.path]
        [void]$signatureRows.Add($row)
    }
    foreach ($name in Get-V7PortableExecutables) {
        $manifest.components.$name = (Get-FileHash -LiteralPath (Join-Path $portableRoot $name) -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    [IO.File]::Delete($portable)
    [IO.Compression.ZipFile]::CreateFromDirectory($unpacked, $portable)
    $manifest.artifacts.exe.sha256 = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    $manifest.artifacts.msi.sha256 = (Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLowerInvariant()
    $manifest.artifacts.portable.sha256 = (Get-FileHash -LiteralPath $portable -Algorithm SHA256).Hash.ToLowerInvariant()
    $utf8NoBom = [Text.UTF8Encoding]::new($false)
    $updates.Add($manifestFullPath, (Join-Path $stage 'manifest.json'))
    [IO.File]::WriteAllText($updates[$manifestFullPath], ($manifest | ConvertTo-Json -Depth 10), $utf8NoBom)

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
    $updates.Add($reportPath, (Join-Path $stage 'signatures.json'))
    [IO.File]::WriteAllText($updates[$reportPath], ($signatureReport | ConvertTo-Json -Depth 8), $utf8NoBom)

    foreach ($destination in $updates.Keys) {
        $pending = $updates[$destination]
        if ([IO.File]::Exists($destination)) {
            [IO.File]::Replace($pending, $destination, "$pending.previous")
        } else {
            [IO.File]::Move($pending, $destination)
        }
        $applied += $destination
    }
} catch {
    $keepBackups = $applied.Count -gt 0
    [array]::Reverse($applied)
    foreach ($destination in $applied) {
        $backup = $updates[$destination] + '.previous'
        if ([IO.File]::Exists($backup)) {
            [IO.File]::Copy($backup, $destination, $true)
        } else {
            [IO.File]::Delete($destination)
        }
    }
    $keepBackups = $false
    throw
} finally {
    if ($keepBackups) { Write-Warning "Rollback was incomplete. Recovery files: $stage" }
    else { Remove-Item -LiteralPath $stage -Recurse -Force }
}
Write-Output "SIGNED_FORMAL_PACKAGE=$root"
Write-Output "SIGNATURE_REPORT=$reportPath"
