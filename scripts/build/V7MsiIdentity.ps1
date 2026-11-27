function New-V7MsiProductCode {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$SourceCommit,
        [Parameter(Mandatory = $true)][string]$PackageTier,
        [Parameter(Mandatory = $true)][string]$ProductVersion,
        [string]$CandidateManifestSha256 = '',
        [string]$BuildNonce = ''
    )

    if ([String]::IsNullOrWhiteSpace($SourceCommit)) {
        throw 'New-V7MsiProductCode requires a non-empty SourceCommit.'
    }
    if ([String]::IsNullOrWhiteSpace($PackageTier)) {
        throw 'New-V7MsiProductCode requires a non-empty PackageTier.'
    }
    if ([String]::IsNullOrWhiteSpace($ProductVersion)) {
        throw 'New-V7MsiProductCode requires a non-empty ProductVersion.'
    }
    $nonce = if ([String]::IsNullOrWhiteSpace($BuildNonce)) {
        [guid]::NewGuid().ToString('N')
    } else {
        $BuildNonce.Trim()
    }
    # Mix commit, package tier, version, candidate manifest digest, and per-build nonce
    # so candidate vs formal packages from the same commit (and repeated builds/repacks)
    # always receive distinct ProductCodes that cleanly trigger Major Upgrade instead of
    # Windows Installer error 1638 (same ProductCode with a new random PackageCode).
    $seed = "hls-downloader-v7-msi-product-code|$($SourceCommit.Trim().ToLowerInvariant())|$($PackageTier.Trim().ToLowerInvariant())|$($ProductVersion.Trim())|$($CandidateManifestSha256.Trim().ToLowerInvariant())|$nonce"
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($seed))
    } finally {
        $sha.Dispose()
    }
    $bytes = New-Object byte[] 16
    [Array]::Copy($hash, 0, $bytes, 0, 16)
    # Format as RFC 4122 version-5-style GUID bytes (version nibble = 5, variant = 10xx).
    $bytes[7] = [byte](($bytes[7] -band 0x0F) -bor 0x50)
    $bytes[8] = [byte](($bytes[8] -band 0x3F) -bor 0x80)
    return ([guid]::new($bytes)).ToString('B').ToUpperInvariant()
}
