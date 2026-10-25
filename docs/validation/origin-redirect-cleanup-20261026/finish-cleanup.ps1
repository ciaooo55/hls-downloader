#requires -Version 7.0
[CmdletBinding(SupportsShouldProcess)]
param([switch]$Apply)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if (-not (Test-Path -LiteralPath (Join-Path $repo 'native_shell\Cargo.toml'))) { throw '未找到项目源码。' }
$delivery = Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader\最新-20261025'
$manifest = Get-Content -LiteralPath (Join-Path $delivery '交付清单.json') -Raw | ConvertFrom-Json
if ($manifest.verified_production_source_commit -ne '1dea557e54b1eb0684065a53368b74c761a1d42d') { throw '桌面交付来源已变化，停止。' }
foreach ($asset in $manifest.assets) {
    $path = [IO.Path]::GetFullPath((Join-Path $delivery $asset.name))
    if (-not $path.StartsWith($delivery + '\', [StringComparison]::OrdinalIgnoreCase) -or
        (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $asset.sha256) {
        throw "桌面成品核对失败：$($asset.name)"
    }
}
if (Get-Process -Name WindowsSandbox,WindowsSandboxClient,HLSDownloader,HLSDownloaderEngine,HLSDownloaderNativeHost,HLSDownloaderPresenter,HLSDownloaderUpdater -ErrorAction SilentlyContinue) {
    throw '沙盒或产品仍在运行，停止清理。'
}
$relativePaths = @('.tool-cache', 'extension\node_modules', 'extension\.output', 'extension\.wxt')
$targets = @()
$sourceLinks = @()
foreach ($relative in $relativePaths) {
    $target = [IO.Path]::GetFullPath((Join-Path $repo $relative))
    if (-not $target.StartsWith($repo + '\', [StringComparison]::OrdinalIgnoreCase)) { throw '目标超出项目。' }
    $tracked = @(git -C $repo ls-files -- $relative.Replace('\', '/'))
    if ($LASTEXITCODE -ne 0 -or $tracked.Count -gt 0) { throw "目标包含源码：$relative" }
    if (-not (Test-Path -LiteralPath $target)) { continue }
    $parent = Get-Item -LiteralPath $target -Force
    while ($parent.FullName -ne $repo) {
        if ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw '目标或父目录为重解析点。' }
        $parent = $parent.Parent
    }
    if ($relative -eq '.tool-cache') {
        foreach ($entry in Get-ChildItem -LiteralPath $target -Force) {
            if ($entry.Name -notin @('audit-fixes', 'build-cache')) { throw "缓存存在非本轮目录：$($entry.Name)" }
        }
    }
    foreach ($link in Get-ChildItem -LiteralPath $target -Directory -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) {
        $resolved = $link.ResolveLinkTarget($true)
        if ($link.LinkType -eq 'SymbolicLink' -and
            $link.Parent.FullName -eq (Join-Path $repo '.tool-cache\build-cache\pnpm-store\v11\projects') -and
            $resolved -and $resolved.FullName -eq (Join-Path $repo 'extension')) {
            $sourceLinks += $link.FullName
        } elseif (-not $resolved -or -not $resolved.FullName.StartsWith($target + '\', [StringComparison]::OrdinalIgnoreCase)) {
            throw "发现目标外链接：$($link.FullName)"
        }
    }
    $targets += $target
}
if (-not $Apply) { $targets; Write-Host '检查完成；未删除文件。加 -Apply 执行。'; return }
foreach ($link in $sourceLinks) {
    if ($PSCmdlet.ShouldProcess($link, '仅移除 pnpm 项目索引符号链接')) { Remove-Item -LiteralPath $link -Force }
}
foreach ($target in $targets) {
    if ($PSCmdlet.ShouldProcess($target, '删除已核对的本轮生成目录')) {
        Remove-Item -LiteralPath $target -Recurse -Force
        Write-Host "已清理：$target"
    }
}
$remaining = @($relativePaths | Where-Object { Test-Path -LiteralPath (Join-Path $repo $_) })
@{
    passed = ($remaining.Count -eq 0)
    generated_paths_remaining = $remaining
    desktop_assets_verified = $manifest.assets.Count
    user_backup_retained = (Test-Path -LiteralPath (Join-Path $repo 'outputs\private-user-backup'))
    host_product_install_performed = $false
    registry_writes_performed = $false
    local_branches = @(git -C $repo branch --format='%(refname:short)')
    worktrees = @(git -C $repo worktree list)
    checked_at = (Get-Date).ToString('o')
} | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $PSScriptRoot 'results\cleanup.json') -Encoding UTF8
if ($remaining.Count -gt 0) { throw '生成目录仍有残留。' }
Write-Host '本轮构建目录已清理；源码、Git、桌面成品、用户备份及下载均保留。'
