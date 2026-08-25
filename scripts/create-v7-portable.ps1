param([string]$OutZip = '')
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
    $readme = "HLS Downloader 7.0.0 Portable`r`n`r`nRun HLSDownloader.exe. Browser Native Messaging registration is repaired automatically on startup. Use scripts\upgrade-v7-portable.ps1 to atomically upgrade another v7 portable folder. The script preserves config.json, data.db and downloads; use -Rollback to restore the previous program image.`r`n"
    [IO.File]::WriteAllText((Join-Path $portable 'README-PORTABLE.txt'), $readme, [Text.UTF8Encoding]::new($false))
    New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($out)) | Out-Null
    Compress-Archive -Path $portable -DestinationPath $out -CompressionLevel Optimal -Force
    $hash = (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash
    Write-Host ("v7 portable created: {0}; SHA-256={1}" -f $out, $hash)
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
