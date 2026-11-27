[CmdletBinding()]
param(
    [string]$ManifestPath = 'artifacts\v7-productization\package\ARTIFACT-MANIFEST.json',
    [string]$CertificateThumbprint = $env:HLS_V7_SIGN_CERT_THUMBPRINT,
    [ValidateSet('CurrentUser', 'LocalMachine')][string]$CertificateStore = $(if ($env:HLS_V7_SIGN_CERT_STORE) { $env:HLS_V7_SIGN_CERT_STORE } else { 'CurrentUser' }),
    [string]$WixRoot = $(if ($env:HLS_V7_WIX_ROOT) { $env:HLS_V7_WIX_ROOT } else { '' }),
    [string]$SignToolPath = $env:HLS_V7_SIGNTOOL,
    [switch]$RequireBurnInnerVerification
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
function Invoke-VerifySignTool([string[]]$Targets) {
    $signArgs = @{
        VerifyOnly = $true
        Path = $Targets
        CertificateThumbprint = $CertificateThumbprint
        CertificateStore = $CertificateStore
    }
    if (-not [String]::IsNullOrWhiteSpace($SignToolPath)) {
        $signArgs['SignToolPath'] = $SignToolPath
    }
    $rows = @(& (Join-Path $PSScriptRoot 'sign-v7-authenticode.ps1') @signArgs | ForEach-Object { $_ | ConvertFrom-Json })
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    return $rows
}

function Export-MsiBinaryStream([string]$MsiFile, [string]$BinaryName, [string]$DestinationPath) {
    $installer = $null
    $database = $null
    $view = $null
    $record = $null
    try {
        $installer = New-Object -ComObject WindowsInstaller.Installer
        $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @([string]$MsiFile, [int]0))
        $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @("SELECT ``Data`` FROM ``Binary`` WHERE ``Name``='$BinaryName'"))
        $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
        $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
        if ($null -eq $record) { throw "MSI Binary table is missing $BinaryName." }
        $size = [int]$record.GetType().InvokeMember('DataSize', 'GetProperty', $null, $record, @([int]1))
        if ($size -le 0) { throw "MSI Binary stream $BinaryName is empty." }
        $bytes = New-Object byte[] $size
        $offset = 0
        while ($offset -lt $size) {
            $chunkSize = [Math]::Min(65536, $size - $offset)
            # Format 1 = msiReadStreamBytes (raw 1:1 byte packing); format 2 (ANSI) corrupts >= 0x80 bytes.
            $chunk = $record.GetType().InvokeMember('ReadStream', 'InvokeMethod', $null, $record, @([int]1, [int]$chunkSize, [int]1))
            if ([String]::IsNullOrEmpty($chunk)) { break }
            for ($idx = 0; $idx -lt $chunk.Length -and $offset -lt $size; $idx++) {
                $bytes[$offset] = [byte]([int]$chunk[$idx] -band 0xFF)
                $offset++
            }
        }
        if ($offset -ne $size) { throw "Truncated MSI Binary stream ${BinaryName}: read $offset of $size bytes." }
        [IO.File]::WriteAllBytes($DestinationPath, $bytes)
    } finally {
        if ($null -ne $record) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) | Out-Null }
        if ($null -ne $view) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) | Out-Null }
        if ($null -ne $database) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) | Out-Null }
        if ($null -ne $installer) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null }
    }
}

$root = Split-Path $manifestFullPath -Parent
$exe = Resolve-V7PackageFile $root $manifest.artifacts.exe.path $manifest.artifacts.exe.sha256
$msi = Resolve-V7PackageFile $root $manifest.artifacts.msi.path $manifest.artifacts.msi.sha256
$portable = Resolve-V7PackageFile $root $manifest.artifacts.portable.path $manifest.artifacts.portable.sha256
$records = @(Invoke-VerifySignTool @($exe, $msi))

$stage = Join-Path $repo ('artifacts\v7-productization\.verify-signature-' + [guid]::NewGuid().ToString('n'))
$burnInnerVerified = $false
try {
    $portableStage = Join-Path $stage 'portable'
    [IO.Compression.ZipFile]::ExtractToDirectory($portable, $portableStage)
    $portableRoot = Join-Path $portableStage 'HLSDownloader'
    $firstParty = @(foreach ($name in Get-V7PortableExecutables) {
        Resolve-V7PackageFile $portableRoot $name $manifest.components.$name
    })
    $records += @(Invoke-VerifySignTool $firstParty)

    # Administrative unpack verifies every first-party EXE embedded in the MSI payload matches
    # the signed manifest SHA-256 and carries a valid Authenticode signature.
    $msiStage = Join-Path $stage 'msi-admin'
    [void][IO.Directory]::CreateDirectory($msiStage)
    $msiLog = Join-Path $stage 'msi-admin.log'
    $msiProc = Start-Process -FilePath 'msiexec.exe' -ArgumentList @('/a', "`"$msi`"", '/qn', "TARGETDIR=`"$msiStage`"", '/L*v', "`"$msiLog`"") -Wait -PassThru -NoNewWindow
    if ($msiProc.ExitCode -ne 0) {
        throw "MSI administrative extraction failed with exit code $($msiProc.ExitCode)."
    }
    $msiRoot = Join-Path $msiStage 'HLSDownloader'
    if (-not (Test-Path -LiteralPath (Join-Path $msiRoot 'HLSDownloader.exe') -PathType Leaf)) {
        $msiRoot = $msiStage
    }
    $msiExecutables = @(foreach ($name in Get-V7MsiFirstPartyExecutables) {
        Resolve-V7PackageFile $msiRoot $name $manifest.components.$name
    })
    [void](Invoke-VerifySignTool $msiExecutables)

    # Verify the MSI embedded Binary\V7InstallerHelper (HLSDownloaderPreflight.exe) signature and digest.
    $msiPreflight = Join-Path $stage 'V7InstallerHelper.exe'
    Export-MsiBinaryStream -MsiFile $msi -BinaryName 'V7InstallerHelper' -DestinationPath $msiPreflight
    $expectedPreflightHash = [string]$manifest.components.'scripts/HLSDownloaderPreflight.exe'
    $actualPreflightHash = (Get-FileHash -LiteralPath $msiPreflight -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualPreflightHash -ne $expectedPreflightHash.ToLowerInvariant()) {
        throw "MSI Binary\V7InstallerHelper SHA-256 mismatch: expected $expectedPreflightHash, got $actualPreflightHash"
    }
    [void](Invoke-VerifySignTool @($msiPreflight))

    # Verify Burn engine signature and embedded MSI digest when WiX insignia/dark are available.
    $cacheRoot = if ($env:HLS_V7_BUILD_CACHE) { [IO.Path]::GetFullPath($env:HLS_V7_BUILD_CACHE) } else { Join-Path $repo '.tool-cache\build-cache' }
    $resolvedWixRoot = if (-not [String]::IsNullOrWhiteSpace($WixRoot)) { [IO.Path]::GetFullPath($WixRoot) } else { Join-Path $cacheRoot 'wix-3.14.1' }
    $insignia = Join-Path $resolvedWixRoot 'insignia.exe'
    $dark = Join-Path $resolvedWixRoot 'dark.exe'
    if ((Test-Path -LiteralPath $insignia -PathType Leaf) -and (Test-Path -LiteralPath $dark -PathType Leaf)) {
        $burnEngine = Join-Path $stage 'burn-engine.exe'
        & $insignia -ib $exe -o $burnEngine -nologo
        if ($LASTEXITCODE -ne 0) { throw "WiX insignia engine extract failed with exit code $LASTEXITCODE." }
        [void](Invoke-VerifySignTool @($burnEngine))

        $burnExtract = Join-Path $stage 'burn-extract'
        [void][IO.Directory]::CreateDirectory($burnExtract)
        & $dark -x $burnExtract $exe -nologo | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "WiX dark bundle extract failed with exit code $LASTEXITCODE." }
        $embeddedMsis = @(Get-ChildItem -LiteralPath $burnExtract -Filter '*.msi' -File -Recurse)
        if ($embeddedMsis.Count -ne 1) {
            throw "Expected exactly one embedded MSI inside Burn bundle; found $($embeddedMsis.Count)."
        }
        $embeddedMsiHash = (Get-FileHash -LiteralPath $embeddedMsis[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($embeddedMsiHash -ne ([string]$manifest.artifacts.msi.sha256).ToLowerInvariant()) {
            throw "Burn embedded MSI SHA-256 mismatch: expected $($manifest.artifacts.msi.sha256), got $embeddedMsiHash"
        }
        [void](Invoke-VerifySignTool @($embeddedMsis[0].FullName))
        $burnInnerVerified = $true
    } elseif ($RequireBurnInnerVerification) {
        throw "Burn inner verification requires WiX insignia.exe and dark.exe under $resolvedWixRoot."
    }
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
Write-Output ([ordered]@{ schema = 1; passed = $true; product_version = [string]$manifest.product_version; source_commit = $currentCommit; signer_thumbprint = [string]$reportJson.signer_thumbprint; msi_inner_verified = $true; burn_inner_verified = $burnInnerVerified } | ConvertTo-Json -Compress)
