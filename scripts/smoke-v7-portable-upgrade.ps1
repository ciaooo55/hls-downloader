param([Parameter(Mandatory=$true)][string]$ArchivePath)
$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) ('hls-v7-portable-smoke-' + [guid]::NewGuid().ToString('n'))
$source = Join-Path $root 'source\HLSDownloader'
$target = Join-Path $root 'target'
$upgradeScript = Join-Path $PSScriptRoot 'upgrade-v7-portable.ps1'
try {
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    Expand-Archive -LiteralPath (Resolve-Path $ArchivePath) -DestinationPath (Join-Path $root 'source') -Force
    Copy-Item -LiteralPath (Join-Path $root 'source\HLSDownloader') -Destination (Join-Path $root 'target') -Recurse -Force
    Set-Content -LiteralPath (Join-Path $target 'v7-old-image.marker') -Value 'old-image' -Encoding ASCII
    New-Item -ItemType Directory -Force -Path (Join-Path $target 'data') | Out-Null
    Set-Content -LiteralPath (Join-Path $target 'data\data.db') -Value 'preserve-db' -Encoding UTF8
    New-Item -ItemType Directory -Force -Path (Join-Path $target 'downloads') | Out-Null
    Set-Content -LiteralPath (Join-Path $target 'downloads\keep.txt') -Value 'preserve-download' -Encoding UTF8
    & $upgradeScript -SourceDir $source -TargetDir $target
    if (-not (Test-Path (Join-Path $target 'HLSDownloader.exe'))) { throw 'upgraded app is missing HLSDownloader.exe' }
    if ((Get-Content (Join-Path $target 'data\data.db') -Raw) -notmatch 'preserve-db') { throw 'database was not preserved' }
    if ((Get-Content (Join-Path $target 'downloads\keep.txt') -Raw) -notmatch 'preserve-download') { throw 'downloads were not preserved' }
    & $upgradeScript -Rollback -RollbackDir $target
    if (-not (Test-Path (Join-Path $target 'v7-old-image.marker'))) { throw 'rollback did not restore old image' }
    Write-Host '{"portable_upgrade":"passed","preserved_state":true,"rollback":true}'
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
