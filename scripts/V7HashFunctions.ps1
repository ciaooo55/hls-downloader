# Windows PowerShell 5.1 host compatibility.
#
# Why this exists: some Windows images ship 5.1 without the binary cmdlet assembly
# (Microsoft.PowerShell.Utility.dll), so Get-FileHash is missing even though the
# module manifest lists it. Every v7 script hashes artifacts with Get-FileHash, so
# dot-sourcing this file defines the missing cmdlet in the caller's scope and keeps
# the pipeline working on both 5.1 and 7.
# Constraint: ASCII-only, because 5.1 reads BOM-less files as GBK.

if (Get-Command Get-FileHash -ErrorAction SilentlyContinue) { return }

function Get-FileHash {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$LiteralPath,
        [ValidateSet('SHA1', 'SHA256', 'SHA384', 'SHA512', 'MD5')]
        [string]$Algorithm = 'SHA256'
    )
    $path = $LiteralPath
    if (-not [IO.Path]::IsPathRooted($path)) {
        $path = [IO.Path]::GetFullPath((Join-Path (Get-Location).Path $path))
    }
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Get-FileHash: file not found: $path"
    }
    $hash = [Security.Cryptography.HashAlgorithm]::Create([string]$Algorithm)
    try {
        $stream = [IO.File]::OpenRead($path)
        try { $bytes = $hash.ComputeHash($stream) } finally { $stream.Dispose() }
    } finally {
        $hash.Dispose()
    }
    $builder = New-Object Text.StringBuilder
    foreach ($b in $bytes) { [void]$builder.Append($b.ToString('x2')) }
    return [pscustomobject]@{
        Algorithm = $Algorithm
        Hash       = $builder.ToString()
        Path       = $path
    }
}

function Get-V7SHA256 {
    [CmdletBinding()]
    param([Parameter(Mandatory = $true)][string]$Path)
    $resolved = if ([IO.Path]::IsPathRooted($Path)) { [IO.Path]::GetFullPath($Path) } else { [IO.Path]::GetFullPath((Join-Path $PWD.Path $Path)) }
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) { throw "file not found: $resolved" }
    $hash = [Security.Cryptography.SHA256]::Create()
    try {
        $stream = [IO.File]::OpenRead($resolved)
        try { $bytes = $hash.ComputeHash($stream) } finally { $stream.Dispose() }
    } finally {
        $hash.Dispose()
    }
    $builder = New-Object Text.StringBuilder
    foreach ($b in $bytes) { [void]$builder.Append($b.ToString('x2')) }
    return $builder.ToString()
}