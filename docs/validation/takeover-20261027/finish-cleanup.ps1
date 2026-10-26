#requires -Version 7.0
[CmdletBinding()]
param([switch]$Apply)
$ErrorActionPreference='Stop'
$repo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..')).TrimEnd('\')
$allowed=@('.tool-cache/takeover-20261027','extension/node_modules','extension/.output','extension/.wxt','extension/docs','extension/.tool-cache')
$records=@(Get-Content (Join-Path $PSScriptRoot 'results/cleanup-inventory.json') -Raw | ConvertFrom-Json)
if (Compare-Object $allowed @($records.path)) { throw 'Unexpected cleanup inventory' }
if (Get-Process WindowsSandbox,WindowsSandboxClient,rustc,cargo,light,HLSDownloader,HLSDownloaderEngine,HLSDownloaderNativeHost,HLSDownloaderPresenter,HLSDownloaderUpdater -ErrorAction SilentlyContinue) {
    throw 'Close Sandbox, product and build processes first; nothing was removed.'
}
$targets=@()
$links=@()
foreach ($record in $records) {
    $path=[IO.Path]::GetFullPath((Join-Path $repo $record.path))
    if (-not $path.StartsWith($repo+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'Target escaped project' }
    if (-not (Test-Path -LiteralPath $path)) { continue }
    $parent=Get-Item -LiteralPath $path -Force
    while ($parent.FullName -ne $repo) {
        if ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Linked cleanup root: $path" }
        $parent=$parent.Parent
    }
    $tracked=@(git -C $repo ls-files -- $record.path)
    if ($LASTEXITCODE -ne 0 -or $tracked.Count) { throw "Tracked source preserved: $($record.path)" }
    $directories=@(Get-ChildItem -LiteralPath $path -Directory -Recurse -Force)
    foreach ($link in $directories | Where-Object {$_.Attributes -band [IO.FileAttributes]::ReparsePoint}) {
        $resolved=$link.ResolveLinkTarget($true)
        if (-not $resolved -or -not $resolved.FullName.StartsWith($repo+'\',[StringComparison]::OrdinalIgnoreCase)) {
            throw "Directory link leaves this project: $($link.FullName)"
        }
        $links+=$link.FullName
    }
    $files=@(Get-ChildItem -LiteralPath $path -File -Recurse -Force)
    $bytes=[double]($files | Measure-Object Length -Sum).Sum
    if ($files.Count -ne $record.files -or $directories.Count -ne $record.directories -or $bytes -ne [double]$record.bytes) {
        throw "Directory changed after inventory; preserved: $($record.path)"
    }
    $targets+=$path
    Write-Host "$($record.path): $($files.Count) files, $bytes bytes"
}
$desktop=Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader/最新-20261025'
$manifest=Get-Content (Join-Path $desktop '交付清单.json') -Raw | ConvertFrom-Json
if ($manifest.verified_production_source_commit -ne 'da06ce46e7f306feaba0d53383f832f3a0179c3c') { throw 'Current delivery source changed' }
foreach ($asset in $manifest.assets) {
    $path=[IO.Path]::GetFullPath((Join-Path $desktop $asset.name))
    if (-not $path.StartsWith([IO.Path]::GetFullPath($desktop)+'\',[StringComparison]::OrdinalIgnoreCase) -or
        (Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -ne $asset.sha256) { throw "Current delivery changed: $($asset.name)" }
}
$oldMsi=Join-Path $desktop 'HLSDownloader-7.0.3-20261026-未签名.msi'
if ((Test-Path -LiteralPath $oldMsi) -and (Get-FileHash -LiteralPath $oldMsi).Hash.ToLowerInvariant() -ne '140b9774ebcb457dd7875b384de055568321a171a493e7cbf2972f50d72fce96') {
    throw 'Old installer was changed; preserved'
}
$previous=Join-Path $PSScriptRoot '../performance-20261027/finish-cleanup.ps1'
& $previous
if (-not $Apply) { Write-Host '只读检查完成。加 -Apply 清理上述生成目录、上一轮缓存和旧安装包；保留源码、Git、最新成品、私有备份及用户下载。'; return }
& $previous -Apply
# pnpm 的项目索引链接指回源码；先非递归删除链接本身，避免清理遍历进入源码。
foreach ($path in $links) { [IO.Directory]::Delete($path,$false) }
foreach ($path in $targets) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction Stop }
if (Test-Path -LiteralPath $oldMsi) { Remove-Item -LiteralPath $oldMsi -Force -ErrorAction Stop }
$cacheParent=Join-Path $repo '.tool-cache'
if ((Test-Path -LiteralPath $cacheParent) -and -not @(Get-ChildItem -LiteralPath $cacheParent -Force).Count) { Remove-Item -LiteralPath $cacheParent -Force }
Write-Host '已清理列明的缓存、空目录和旧安装包；最新成品、完整源码和用户数据保留。'
