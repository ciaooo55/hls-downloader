param(
    [string]$OutZip = '',
    [string]$AppImage = ''
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\V7HashFunctions.ps1')
. (Join-Path $PSScriptRoot '..\V7ExtensionArchive.ps1')
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$composeBuild = if ($env:HLS_COMPOSE_BUILD_DIR) { $env:HLS_COMPOSE_BUILD_DIR } else { Join-Path $repo '.tool-cache\build-cache\compose-build' }
$appImage = if ($AppImage) { (Resolve-Path -LiteralPath $AppImage).Path } else { Join-Path $composeBuild 'compose\binaries\main\app\HLSDownloader' }
if (-not (Test-Path -LiteralPath (Join-Path $appImage 'HLSDownloader.exe'))) {
    throw "Application image is missing: $appImage. Build the selected runtime first."
}
$provenancePath = Join-Path $appImage 'app\resources\BUILD-PROVENANCE.json'
$featureParityPath = Join-Path $appImage 'app\resources\FEATURE-PARITY.json'
if (-not (Test-Path -LiteralPath $provenancePath -PathType Leaf) -or
    -not (Test-Path -LiteralPath $featureParityPath -PathType Leaf)) {
    throw 'Application image is missing the v7 provenance or feature parity files.'
}
$provenance = Get-Content -LiteralPath $provenancePath -Raw -Encoding UTF8 | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
$canonicalFeatureParity = Join-Path $repo 'config\release\feature-parity.json'
$productVersion = [string](Get-Content -LiteralPath $canonicalFeatureParity -Raw -Encoding UTF8 | ConvertFrom-Json).product_version
$canonicalFeatureHash = (Get-FileHash -LiteralPath $canonicalFeatureParity -Algorithm SHA256).Hash.ToLowerInvariant()
$embeddedFeatureHash = (Get-FileHash -LiteralPath $featureParityPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ([int]$provenance.schema -ne 1 -or
    [string]$provenance.product_version -ne $productVersion -or
    @('candidate', 'formal') -notcontains [string]$provenance.package_tier -or
    [string]$provenance.source_commit -ne $currentCommit -or
    [string]$provenance.source_tree -ne $currentTree -or
    [string]$provenance.feature_parity_path -ne 'config/release/feature-parity.json' -or
    [string]$provenance.feature_parity_sha256 -ne $canonicalFeatureHash -or
    $embeddedFeatureHash -ne $canonicalFeatureHash) {
    throw 'Application image provenance is not bound to the current v7 source and feature parity.'
}
$out = if ($OutZip) { [IO.Path]::GetFullPath($OutZip) } else { Join-Path $repo "artifacts\v7-productization\package\HLSDownloader-$productVersion-Windows-x64-Portable.zip" }
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
try {
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item -LiteralPath $appImage -Destination $stage -Recurse -Force
    New-Item -ItemType Directory -Force -Path (Join-Path $portable 'scripts') | Out-Null
    New-Item -ItemType File -Force -Path (Join-Path $portable 'portable') | Out-Null
    Copy-Item -LiteralPath (Join-Path $repo 'scripts\upgrade-v7-portable.ps1') -Destination (Join-Path $portable 'scripts\upgrade-v7-portable.ps1') -Force
    Copy-Item -LiteralPath (Join-Path $repo 'scripts\V7HashFunctions.ps1') -Destination (Join-Path $portable 'scripts\V7HashFunctions.ps1') -Force
    $rustCommand = Get-Command rustc.exe -ErrorAction SilentlyContinue
    $rustc = if ($rustCommand) { $rustCommand.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\rustc.exe' }
    $preflight = Join-Path $portable 'scripts\HLSDownloaderPreflight.exe'
    & $rustc --edition 2021 --crate-name hls_installer_preflight -C opt-level=s -C panic=abort -C target-feature=+crt-static (Join-Path $repo 'scripts\install\installer-preflight.rs') -o $preflight
    if ($LASTEXITCODE -ne 0) { throw "Portable preflight build failed: $LASTEXITCODE" }
    $portableExtensions = Join-Path $portable 'extensions'
    New-Item -ItemType Directory -Force -Path $portableExtensions | Out-Null
    $packagedExtensions = Join-Path $appImage 'app\resources\extensions'
    foreach ($item in @(
        @{ Browser = 'Chromium'; Name = "HLSDownloader-$productVersion-Chromium.zip" },
        @{ Browser = 'Firefox'; Name = "HLSDownloader-$productVersion-Firefox.zip" }
    )) {
        $archive = Join-Path $packagedExtensions $item.Name
        if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
            throw "Packaged $($item.Browser) extension archive is missing from the App-Image: $archive"
        }
        Assert-V7ExtensionArchive -Archive $archive -Browser $item.Browser -Version $productVersion -ChromiumKey $expectedChromiumKey -FirefoxId $expectedFirefoxId
        Copy-Item -LiteralPath $archive -Destination (Join-Path $portableExtensions $item.Name) -Force
    }
    $readme = "HLS Downloader $productVersion Portable`r`n`r`nRun HLSDownloader.exe. Runtime data stays in data; completed files and resumable task state stay in downloads. Browser Native Messaging registration is repaired automatically on startup. The extensions folder contains the matching Chromium and Firefox MV3 packages. Use scripts\upgrade-v7-portable.ps1 to atomically upgrade another v7 portable folder while preserving data and downloads; use -Rollback to restore the previous program image.`r`n"
    [IO.File]::WriteAllText((Join-Path $portable 'README-PORTABLE.txt'), $readme, [Text.UTF8Encoding]::new($false))
    New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($out)) | Out-Null
    Compress-Archive -Path $portable -DestinationPath $out -CompressionLevel Optimal -Force
    $hash = (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash
    Write-Host ("v7 portable created: {0}; SHA-256={1}" -f $out, $hash)
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
