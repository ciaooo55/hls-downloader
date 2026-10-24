$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw '请使用 PowerShell 7 执行此脚本。' }
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if (-not (Test-Path -LiteralPath (Join-Path $repo 'native_shell\Cargo.toml'))) {
    throw '未找到项目源码，停止清理。'
}
$installer = Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader\最新-20261024\HLSDownloader-7.0.3-20261024-未签名.msi'
if (-not (Test-Path -LiteralPath $installer)) { throw '桌面交付包不存在，停止清理。' }
if ((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant() -ne '7aa26f74fbc1e4db6f0cf1fba1a52eaeb26c78a479e56f31f67f95c56206ec8d') {
    throw '桌面安装包与本轮验收包不同，停止清理。'
}
if (Get-Process -Name WindowsSandbox,WindowsSandboxClient -ErrorAction SilentlyContinue) {
    throw '请先关闭 Windows 沙盒，避免清理正在使用的映射文件。'
}
$relativePaths = @(
    '.tool-cache\response-intercept-20261024'
    '.tool-cache\browser-takeover-20261024'
    '.tool-cache\interleaved-playback-20261024'
    'extension\node_modules'
    'extension\.output'
    'extension\.wxt'
    'extension\.tool-cache\browser-takeover-20261024'
    '.tool-cache\release-upload'
)
$targets = @()
$storeLinks = @()
foreach ($relative in $relativePaths) {
    $target = [IO.Path]::GetFullPath((Join-Path $repo $relative))
    if (-not $target.StartsWith($repo + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw '清理目标超出项目，停止。'
    }
    if (-not (Test-Path -LiteralPath $target)) { continue }
    $parent = Get-Item -LiteralPath $target -Force
    while ($parent.FullName -ne $repo) {
        if ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "发现重解析目录：$($parent.FullName)" }
        $parent = $parent.Parent
    }
    foreach ($link in Get-ChildItem -LiteralPath $target -Directory -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) {
        # pnpm 的包链接留在自己的 node_modules 内；PS 7 删除链接本身，不递归其目标。
        $resolved = $link.ResolveLinkTarget($true)
        $storeProjects = Join-Path $target 'pnpm-store\v11\projects'
        if ($relative -eq '.tool-cache\browser-takeover-20261024' -and
            $link.LinkType -eq 'SymbolicLink' -and
            $link.Parent.FullName -eq $storeProjects -and
            $resolved -and $resolved.FullName -eq (Join-Path $repo 'extension')) {
            $storeLinks += $link.FullName
            continue
        }
        if ($relative -ne 'extension\node_modules' -or -not $resolved -or -not $resolved.FullName.StartsWith($target + '\', [StringComparison]::OrdinalIgnoreCase)) {
            throw "发现范围外的重解析目录：$($link.FullName)"
        }
    }
    $targets += $target
}
foreach ($link in $storeLinks) {
    # pnpm 的项目索引指回源码；只删链接本身，不能递归链接目标。
    Remove-Item -LiteralPath $link -Force
}
foreach ($target in $targets) {
    Remove-Item -LiteralPath $target -Recurse -Force
    Write-Host "已清理：$target"
}
Write-Host '源码、Git、桌面成品、用户下载和备份均保留。'
