#requires -Version 7.0
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..')).TrimEnd('\')
if ($root -ne 'A:\Ubuntu\测试\hls-downloader' -or -not (Test-Path -LiteralPath (Join-Path $root '.git'))) {
    throw 'Unexpected project root; nothing was deleted.'
}
$delivery = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'desktop-artifacts.json') -Raw | ConvertFrom-Json
foreach ($item in $delivery) {
    if ((Get-FileHash -LiteralPath $item.path -Algorithm SHA256).Hash -ne $item.sha256) {
        throw "Delivery hash mismatch: $($item.path)"
    }
}
$oldRoot = 'C:\Users\lee\Desktop\HLSDownloader\最新-46d09b5'
if (Test-Path -LiteralPath $oldRoot) {
    $old = (Get-Content -LiteralPath (Join-Path $root 'docs\validation\download-ui-20261019\result.json') -Raw | ConvertFrom-Json).delivery_files
    $actual = @(Get-ChildItem -LiteralPath $oldRoot -Force)
    if ($actual.Count -ne $old.Count) { throw 'Unexpected old delivery content.' }
    foreach ($item in $old) {
        $path = [IO.Path]::GetFullPath((Join-Path $oldRoot $item.name))
        if ([IO.Path]::GetDirectoryName($path) -ne $oldRoot -or
            (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $item.sha256) {
            throw 'Old delivery changed; nothing was removed.'
        }
    }
    foreach ($item in $old) { Remove-Item -LiteralPath (Join-Path $oldRoot $item.name) -Force }
    Remove-Item -LiteralPath $oldRoot
    Write-Output "已清理旧交付：$oldRoot"
}
$generated = @('.tool-cache', 'desktop_ui\.gradle', 'desktop_ui\.kotlin',
    'desktop_ui\.tool-cache', 'desktop_ui\build', 'extension\.output',
    'extension\.wxt', 'extension\node_modules', 'scripts\validation\__pycache__')
foreach ($relative in $generated) {
    $target = [IO.Path]::GetFullPath((Join-Path $root $relative))
    if (-not $target.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Cleanup target escaped project root.'
    }
    $tracked = @(& git -C $root ls-files -- $relative)
    if ($LASTEXITCODE -ne 0 -or $tracked.Count -gt 0) { throw "Tracked files in $relative" }
    if (Test-Path -LiteralPath $target) {
        if ((Get-Item -LiteralPath $target -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Cleanup root is a link: $target"
        }
        Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction Stop
        Write-Output "已清理：$target"
    }
}
Write-Output '完成：保留源码、Git、桌面四份交付包；未操作宿主安装、下载或注册表。'
