param(
    [string]$OutZip = ''
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$appImage = 'D:\HLSDownloaderBuildCache\compose-build\compose\binaries\main\app\HLSDownloader'
if (-not (Test-Path -LiteralPath (Join-Path $appImage 'HLSDownloader.exe'))) {
    throw "Compose App-Image is missing: $appImage. Run gradlew.bat createDistributable first."
}
$provenancePath = Join-Path $appImage 'app\resources\BUILD-PROVENANCE.json'
$featureParityPath = Join-Path $appImage 'app\resources\FEATURE-PARITY.json'
if (-not (Test-Path -LiteralPath $provenancePath -PathType Leaf) -or
    -not (Test-Path -LiteralPath $featureParityPath -PathType Leaf)) {
    throw 'Compose App-Image is missing the v7 provenance or feature parity files.'
}
$provenance = Get-Content -LiteralPath $provenancePath -Raw -Encoding UTF8 | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse HEAD^{tree}).Trim()
$canonicalFeatureParity = Join-Path $repo 'artifacts\v7-productization\feature-parity.json'
$canonicalFeatureHash = (Get-FileHash -LiteralPath $canonicalFeatureParity -Algorithm SHA256).Hash.ToLowerInvariant()
$embeddedFeatureHash = (Get-FileHash -LiteralPath $featureParityPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ([int]$provenance.schema -ne 1 -or
    [string]$provenance.product_version -ne '7.0.0' -or
    [string]$provenance.source_commit -ne $currentCommit -or
    [string]$provenance.source_tree -ne $currentTree -or
    [string]$provenance.feature_parity_path -ne 'artifacts/v7-productization/feature-parity.json' -or
    [string]$provenance.feature_parity_sha256 -ne $canonicalFeatureHash -or
    $embeddedFeatureHash -ne $canonicalFeatureHash) {
    throw 'Compose App-Image provenance is not bound to the current v7 source and feature parity.'
}
$out = if ($OutZip) { [IO.Path]::GetFullPath($OutZip) } else { Join-Path $repo 'artifacts\v7-productization\package\HLSDownloader-7.0.0-Windows-x64-Portable.zip' }
$repoPrefix = $repo.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
if (-not $out.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Portable output must stay inside this repository: $out"
}
$stage = Join-Path $repo ('artifacts\v7-productization\.portable-stage-' + [guid]::NewGuid().ToString('n'))
$portable = Join-Path $stage 'HLSDownloader'
$identitySource = Get-Content -LiteralPath (Join-Path $repo 'extension\lib\storeIdentity.ts') -Raw -Encoding UTF8
$expectedChromiumKey = ([regex]::Match($identitySource, "CHROMIUM_PUBLIC_KEY = '([^']+)'" )).Groups[1].Value
$expectedFirefoxId = ([regex]::Match($identitySource, "FIREFOX_EXTENSION_ID = '([^']+)'" )).Groups[1].Value
if ([String]::IsNullOrWhiteSpace($expectedChromiumKey) -or [String]::IsNullOrWhiteSpace($expectedFirefoxId)) {
    throw 'Extension store identity constants are missing.'
}
function Assert-ExtensionArchive([string]$Archive, [string]$Browser) {
    $check = Join-Path $stage ('.extension-check-' + $Browser)
    try {
        Expand-Archive -LiteralPath $Archive -DestinationPath $check -Force
        $manifestPath = Join-Path $check 'manifest.json'
        if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
            throw "$Browser extension archive is missing manifest.json."
        }
        $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($manifest.version -ne '7.0.0') {
            throw "$Browser extension manifest version is not 7.0.0: $($manifest.version)"
        }
        if ([int]$manifest.manifest_version -ne 3) {
            throw "$Browser extension is not Manifest V3."
        }
        if ($Browser -eq 'Chromium' -and [string]$manifest.key -ne $expectedChromiumKey) {
            throw 'Chromium extension key does not match store identity.'
        }
        if ($Browser -eq 'Firefox' -and [string]$manifest.browser_specific_settings.gecko.id -ne $expectedFirefoxId) {
            throw 'Firefox extension id does not match store identity.'
        }
    } finally {
        Remove-Item -LiteralPath $check -Recurse -Force -ErrorAction SilentlyContinue
    }
}
try {
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item -LiteralPath $appImage -Destination $stage -Recurse -Force
    New-Item -ItemType Directory -Force -Path (Join-Path $portable 'scripts') | Out-Null
    New-Item -ItemType File -Force -Path (Join-Path $portable 'portable') | Out-Null
    Copy-Item -LiteralPath (Join-Path $repo 'scripts\upgrade-v7-portable.ps1') -Destination (Join-Path $portable 'scripts\upgrade-v7-portable.ps1') -Force
    $portableExtensions = Join-Path $portable 'extensions'
    New-Item -ItemType Directory -Force -Path $portableExtensions | Out-Null
    $packagedExtensions = Join-Path $appImage 'app\resources\extensions'
    foreach ($item in @(
        @{ Browser = 'Chromium'; Name = 'HLSDownloader-7.0.0-Chromium.zip' },
        @{ Browser = 'Firefox'; Name = 'HLSDownloader-7.0.0-Firefox.zip' }
    )) {
        $archive = Join-Path $packagedExtensions $item.Name
        if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
            throw "Packaged $($item.Browser) extension archive is missing from the App-Image: $archive"
        }
        Assert-ExtensionArchive $archive $item.Browser
        Copy-Item -LiteralPath $archive -Destination (Join-Path $portableExtensions $item.Name) -Force
    }
    $readme = "HLS Downloader 7.0.0 Portable`r`n`r`nRun HLSDownloader.exe. Browser Native Messaging registration is repaired automatically on startup. The extensions folder contains the matching Chromium and Firefox MV3 packages. Use scripts\upgrade-v7-portable.ps1 to atomically upgrade another v7 portable folder. The script preserves config.json, data.db and downloads; use -Rollback to restore the previous program image.`r`n"
    [IO.File]::WriteAllText((Join-Path $portable 'README-PORTABLE.txt'), $readme, [Text.UTF8Encoding]::new($false))
    New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($out)) | Out-Null
    Compress-Archive -Path $portable -DestinationPath $out -CompressionLevel Optimal -Force
    $hash = (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash
    Write-Host ("v7 portable created: {0}; SHA-256={1}" -f $out, $hash)
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
