#requires -Version 7.0
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if (-not (Test-Path -LiteralPath (Join-Path $repo 'native_shell\Cargo.toml'))) {
    throw '未找到本项目源码，停止清理。'
}
$delivery = Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader\最新-20261025'
$installer = Join-Path $delivery 'HLSDownloader-7.0.3-20261025-未签名.msi'
if (-not (Test-Path -LiteralPath $installer) -or
    (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'b1c1010f56896d3351ecc590f83c729b5ec25bd0a0c8ec0f179d3c4dc14c5afd') {
    throw '桌面成品不存在或哈希不同，停止清理。'
}
if (Get-Process -Name WindowsSandbox,WindowsSandboxClient -ErrorAction SilentlyContinue) {
    throw '请先关闭 Windows 沙盒，避免清理正在使用的映射文件。'
}
$scratch = Join-Path $repo '.tool-cache\live-presenter-20261024'
$relativePaths = @(
    '.tool-cache\request-control-20261025'
    '.tool-cache\http-transport-20261025'
    '.tool-cache\build-cache'
    '.tool-cache\navigation-20261025'
    'desktop_ui\.gradle'
    'desktop_ui\.kotlin'
    'extension\node_modules'
    'extension\.output'
    'extension\.wxt'
    '.tool-cache\live-presenter-20261024'
)
$targets = @()
$sourceLinks = @()
foreach ($relative in $relativePaths) {
    $target = [IO.Path]::GetFullPath((Join-Path $repo $relative))
    if (-not $target.StartsWith($repo + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw '清理目标超出项目，停止。'
    }
    if (-not (Test-Path -LiteralPath $target)) { continue }
    $parent = Get-Item -LiteralPath $target -Force
    while ($parent.FullName -ne $repo) {
        if ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "目标或父目录是重解析点：$($parent.FullName)"
        }
        $parent = $parent.Parent
    }
    foreach ($link in Get-ChildItem -LiteralPath $target -Directory -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) {
        $resolved = $link.ResolveLinkTarget($true)
        # pnpm 项目索引指回源码，只允许删除链接本身，不能沿链接删除源码。
        if ($target -eq $scratch -and $link.LinkType -eq 'SymbolicLink' -and
            $link.Parent.FullName -eq (Join-Path $scratch 'pnpm-store\v11\projects') -and
            $resolved -and $resolved.FullName -eq (Join-Path $repo 'extension')) {
            $sourceLinks += $link.FullName
            continue
        }
        if (-not $resolved -or -not $resolved.FullName.StartsWith($target + '\', [StringComparison]::OrdinalIgnoreCase)) {
            throw "发现目标外的链接：$($link.FullName)"
        }
    }
    $targets += $target
}
foreach ($link in $sourceLinks) { Remove-Item -LiteralPath $link -Force }
foreach ($target in $targets) {
    Remove-Item -LiteralPath $target -Recurse -Force
    Write-Host "已清理：$target"
}
Write-Host '本轮生成目录已清理；源码、Git、桌面成品、宿主安装、用户下载及备份均保留。'
