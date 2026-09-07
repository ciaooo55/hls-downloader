[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateNotNullOrEmpty()][string[]]$Path,
    [string]$CertificateThumbprint = $env:HLS_V7_SIGN_CERT_THUMBPRINT,
    [ValidateSet('CurrentUser', 'LocalMachine')][string]$CertificateStore = $(if ($env:HLS_V7_SIGN_CERT_STORE) { $env:HLS_V7_SIGN_CERT_STORE } else { 'CurrentUser' }),
    [string]$TimestampUrl = $(if ($env:HLS_V7_TIMESTAMP_URL) { $env:HLS_V7_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }),
    [string]$SignToolPath = $env:HLS_V7_SIGNTOOL,
    [switch]$VerifyOnly
)

$ErrorActionPreference = 'Stop'
$thumbprint = ($CertificateThumbprint -replace '\s', '').ToUpperInvariant()
if ($thumbprint -notmatch '^[0-9A-F]{40,64}$') {
    throw 'HLS_V7_SIGN_CERT_THUMBPRINT must contain a certificate thumbprint.'
}

function Resolve-SignTool([string]$ExplicitPath) {
    if (-not [String]::IsNullOrWhiteSpace($ExplicitPath)) {
        $resolved = (Resolve-Path -LiteralPath $ExplicitPath).Path
        if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) { throw "signtool.exe is missing: $resolved" }
        return $resolved
    }
    $command = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    if (Test-Path -LiteralPath $kitsRoot -PathType Container) {
        $candidate = Get-ChildItem -LiteralPath $kitsRoot -Filter signtool.exe -File -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
            Sort-Object FullName -Descending |
            Select-Object -First 1
        if ($candidate) { return $candidate.FullName }
    }
    throw 'signtool.exe was not found. Install a Windows SDK or set HLS_V7_SIGNTOOL.'
}

function Resolve-Certificate([string]$Scope, [string]$ExpectedThumbprint, [bool]$RequirePrivateKey) {
    $certificatePath = "Cert:\$Scope\My\$ExpectedThumbprint"
    $certificate = Get-Item -LiteralPath $certificatePath -ErrorAction SilentlyContinue
    if (-not $certificate) { throw "Signing certificate was not found at $certificatePath." }
    if ($RequirePrivateKey -and -not $certificate.HasPrivateKey) { throw 'Signing certificate does not expose a private key to the release runner.' }
    if ((Get-Date) -lt $certificate.NotBefore -or (Get-Date) -gt $certificate.NotAfter) { throw 'Signing certificate is not currently valid.' }
    if (@($certificate.Extensions | Where-Object { $_.Oid.Value -eq '2.5.29.37' -and $_.Format($false) -notmatch 'Code Signing' }).Count -gt 0) {
        throw 'Configured certificate does not advertise Code Signing usage.'
    }
    return $certificate
}

$signTool = Resolve-SignTool $SignToolPath
$certificate = Resolve-Certificate $CertificateStore $thumbprint (-not $VerifyOnly)
$files = @($Path | ForEach-Object { (Resolve-Path -LiteralPath $_).Path } | Select-Object -Unique)
if ($files.Count -eq 0) { throw 'No Authenticode targets were supplied.' }

foreach ($file in $files) {
    if (-not $VerifyOnly) {
        $arguments = @('sign', '/fd', 'SHA256', '/td', 'SHA256', '/tr', $TimestampUrl, '/sha1', $thumbprint)
        if ($CertificateStore -eq 'LocalMachine') { $arguments += '/sm' }
        $arguments += $file
        & $signTool @arguments
        if ($LASTEXITCODE -ne 0) { throw "signtool sign failed for $file with exit code $LASTEXITCODE." }
    }

    & $signTool verify /pa /all /v $file
    if ($LASTEXITCODE -ne 0) { throw "signtool verify failed for $file with exit code $LASTEXITCODE." }
    $signature = Get-AuthenticodeSignature -LiteralPath $file
    if ($signature.Status -ne 'Valid') { throw "Authenticode status is $($signature.Status) for $file." }
    if (-not $signature.SignerCertificate -or $signature.SignerCertificate.Thumbprint.ToUpperInvariant() -ne $certificate.Thumbprint.ToUpperInvariant()) {
        throw "Unexpected Authenticode signer for $file."
    }
    if (-not $signature.TimeStamperCertificate) { throw "Authenticode timestamp is missing for $file." }
    Write-Output ([ordered]@{
        path = $file
        signer_thumbprint = $signature.SignerCertificate.Thumbprint.ToLowerInvariant()
        timestamp_thumbprint = $signature.TimeStamperCertificate.Thumbprint.ToLowerInvariant()
        status = [string]$signature.Status
    } | ConvertTo-Json -Compress)
}
