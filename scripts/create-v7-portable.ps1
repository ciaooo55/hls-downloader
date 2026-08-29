param(
    [string]$OutZip = '',
    [string]$ExtensionOutput = ''
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$appImage = 'D:\HLSDownloaderBuildCache\compose-build\compose\binaries\main\app\HLSDownloader'
if (-not (Test-Path -LiteralPath (Join-Path $appImage 'HLSDownloader.exe'))) {
    throw "Compose App-Image is missing: $appImage. Run gradlew.bat createDistributable first."
}
$out = if ($OutZip) { [IO.Path]::GetFullPath($OutZip) } else { Join-Path $repo 'artifacts\v7-productization\package\HLSDownloader-7.0.0-Windows-x64-Portable.zip' }
$stage = Join-Path ([IO.Path]::GetTempPath()) ('hls-v7-portable-stage-' + [guid]::NewGuid().ToString('n'))
$portable = Join-Path $stage 'HLSDownloader'
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
        @{ Browser = 'Chromium'; Name = 'HLSDownloader-7.0.0-Chromium.zip'; Source = 'chrome-mv3' },
        @{ Browser = 'Firefox'; Name = 'HLSDownloader-7.0.0-Firefox.zip'; Source = 'firefox-mv3' }
    )) {
        $archive = Join-Path $packagedExtensions $item.Name
        if (Test-Path -LiteralPath $archive -PathType Leaf) {
            Copy-Item -LiteralPath $archive -Destination (Join-Path $portableExtensions $item.Name) -Force
            continue
        }
        $outputRoot = if ($ExtensionOutput) { [IO.Path]::GetFullPath($ExtensionOutput) } else { Join-Path $repo 'extension\.output' }
        $source = Join-Path $outputRoot $item.Source
        if (-not (Test-Path -LiteralPath (Join-Path $source 'manifest.json') -PathType Leaf)) {
            throw "Production browser extension is missing: $source"
        }
        $manifest = Get-Content -LiteralPath (Join-Path $source 'manifest.json') -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($manifest.version -ne '7.0.0') { throw "Built $($item.Browser) extension version is not 7.0.0: $($manifest.version)" }
        Compress-Archive -Path (Join-Path $source '*') -DestinationPath (Join-Path $portableExtensions $item.Name) -CompressionLevel Optimal -Force
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
