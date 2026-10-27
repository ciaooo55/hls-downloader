#requires -Version 7.0
[CmdletBinding()]
param([switch]$Apply)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..')).TrimEnd('\')
if (-not (Test-Path -LiteralPath (Join-Path $repo 'native_shell/Cargo.toml'))) { throw 'Project source missing' }
$target = [IO.Path]::GetFullPath((Join-Path $repo '.tool-cache/performance-20261027'))
if ($target -ne "$repo\.tool-cache\performance-20261027") { throw 'Unexpected cleanup target' }
if (-not (Test-Path -LiteralPath $target)) { Write-Host '本轮缓存已不存在。'; return }
if (Get-Process WindowsSandbox,WindowsSandboxClient,rustc,cargo,HLSDownloader,HLSDownloaderEngine,HLSDownloaderNativeHost,HLSDownloaderPresenter,HLSDownloaderUpdater -ErrorAction SilentlyContinue) {
    throw 'Close Sandbox, product and build processes before cleanup; nothing was removed.'
}
$tracked = @(git -C $repo ls-files -- '.tool-cache/performance-20261027')
if ($LASTEXITCODE -ne 0 -or $tracked.Count) { throw 'Cannot confirm cache contains no tracked source' }
$parent = Get-Item -LiteralPath $target -Force
while ($parent.FullName -ne $repo) {
    if ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Cleanup path has a reparse point' }
    $parent = $parent.Parent
}
if (@(Get-ChildItem -LiteralPath $target -Directory -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }).Count) {
    throw 'Cache contains a directory link; nothing was removed.'
}
$record = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'results/cleanup.json') -Raw | ConvertFrom-Json
$files = @(Get-ChildItem -LiteralPath $target -File -Recurse -Force)
$bytes = ($files | Measure-Object -Property Length -Sum).Sum
if ($record.path -ne '.tool-cache/performance-20261027' -or $files.Count -ne $record.files -or $bytes -ne $record.bytes) {
    throw 'Cache contents changed after inventory; nothing was removed.'
}
$delivery = Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader/最新-20261025'
$manifest = Get-Content -LiteralPath (Join-Path $delivery '交付清单.json') -Raw | ConvertFrom-Json
if ($manifest.verified_production_source_commit -ne 'd27804fb3d5ab17b710455fdbc684b31ed42fd80') { throw 'Delivery source changed' }
foreach ($asset in $manifest.assets) {
    $path = [IO.Path]::GetFullPath((Join-Path $delivery $asset.name))
    if (-not $path.StartsWith([IO.Path]::GetFullPath($delivery) + '\', [StringComparison]::OrdinalIgnoreCase) -or
        (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $asset.sha256) {
        throw "Delivery differs: $($asset.name)"
    }
}
Write-Host "目标：$target；$($files.Count) 个文件，$bytes 字节。"
if (-not $Apply) { Write-Host '只读检查完成；加 -Apply 执行清理。'; return }
Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction Stop
$cacheParent = Join-Path $repo '.tool-cache'
if (-not @(Get-ChildItem -LiteralPath $cacheParent -Force).Count) { Remove-Item -LiteralPath $cacheParent -Force }
Write-Host '本轮缓存已清理；源码、Git、桌面成品、私有备份和用户下载保留。'
