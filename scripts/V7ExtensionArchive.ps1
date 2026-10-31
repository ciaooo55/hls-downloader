# Shared validation for the packaged browser extensions. ASCII-only for PS 5.1.
function Assert-V7ExtensionArchive {
    param(
        [Parameter(Mandatory = $true)][string]$Archive,
        [Parameter(Mandatory = $true)][ValidateSet('Chromium', 'Firefox')][string]$Browser,
        [Parameter(Mandatory = $true)][string]$Version,
        [Parameter(Mandatory = $true)][string]$ChromiumKey,
        [Parameter(Mandatory = $true)][string]$FirefoxId
    )
    if (-not (Test-Path -LiteralPath $Archive -PathType Leaf)) {
        throw "Packaged $Browser extension archive is missing: $Archive"
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead([IO.Path]::GetFullPath($Archive))
    try {
        $entry = $zip.GetEntry('manifest.json')
        if ($null -eq $entry) { throw "Packaged $Browser extension archive is missing manifest.json." }
        $reader = New-Object IO.StreamReader($entry.Open())
        try { $manifest = $reader.ReadToEnd() | ConvertFrom-Json } finally { $reader.Dispose() }
        if ($manifest.version -ne $Version) {
            throw "Packaged $Browser extension version is not ${Version}: $($manifest.version)"
        }
        if ([int]$manifest.manifest_version -ne 3) {
            throw "Packaged $Browser extension is not Manifest V3."
        }
        if ($Browser -eq 'Chromium' -and [string]$manifest.key -ne $ChromiumKey) {
            throw 'Packaged Chromium extension key does not match store identity.'
        }
        if ($Browser -eq 'Firefox' -and [string]$manifest.browser_specific_settings.gecko.id -ne $FirefoxId) {
            throw 'Packaged Firefox extension id does not match store identity.'
        }
    } finally { $zip.Dispose() }
}
