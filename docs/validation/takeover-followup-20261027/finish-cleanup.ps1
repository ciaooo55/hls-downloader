#requires -Version 7.0
[CmdletBinding()]
param([switch]$Apply)
$ErrorActionPreference='Stop'
$repo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..')).TrimEnd('\')
$record=Get-Content (Join-Path $PSScriptRoot 'results/cleanup-inventory.json') -Raw | ConvertFrom-Json
if ($record.path -ne '.tool-cache/takeover-followup-20261027') { throw 'Unexpected cleanup target' }
$target=[IO.Path]::GetFullPath((Join-Path $repo $record.path))
if ($target -ne "$repo\.tool-cache\takeover-followup-20261027") { throw 'Cleanup escaped project' }
if (Get-Process WindowsSandbox,WindowsSandboxClient,rustc,cargo,light,HLSDownloader,HLSDownloaderEngine,HLSDownloaderNativeHost,HLSDownloaderPresenter,HLSDownloaderUpdater -ErrorAction SilentlyContinue) {
    throw 'Close Sandbox, product and build processes first; nothing was removed.'
}
if (Test-Path -LiteralPath $target) {
    $parent=Get-Item -LiteralPath $target -Force
    while ($parent.FullName -ne $repo) {
        if ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Cleanup root is linked' }
        $parent=$parent.Parent
    }
    $directories=@(Get-ChildItem -LiteralPath $target -Directory -Recurse -Force)
    if (@($directories | Where-Object {$_.Attributes -band [IO.FileAttributes]::ReparsePoint}).Count) { throw 'Cache contains directory links' }
    $files=@(Get-ChildItem -LiteralPath $target -File -Recurse -Force)
    $bytes=[double]($files | Measure-Object Length -Sum).Sum
    if ($files.Count -ne $record.files -or $directories.Count -ne $record.directories -or $bytes -ne [double]$record.bytes) {
        throw 'Cache changed after inventory; nothing was removed.'
    }
    $tracked=@(git -C $repo ls-files -- $record.path)
    if ($LASTEXITCODE -ne 0 -or $tracked.Count) { throw 'Tracked source preserved' }
    Write-Host "$($record.path): $($files.Count) files, $bytes bytes"
}
$previous=Join-Path $PSScriptRoot '../takeover-20261027/finish-cleanup.ps1'
& $previous
if (-not $Apply) { Write-Host '只读检查完成。加 -Apply 清理列明缓存和旧包；源码、Git、最新桌面成品、私有备份和下载保留。'; return }
& $previous -Apply
if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction Stop }
$cache=Join-Path $repo '.tool-cache'
if ((Test-Path -LiteralPath $cache) -and -not @(Get-ChildItem -LiteralPath $cache -Force).Count) { Remove-Item -LiteralPath $cache -Force }
Write-Host '列明缓存和旧包已清理；最新成品与完整源码保留。'
