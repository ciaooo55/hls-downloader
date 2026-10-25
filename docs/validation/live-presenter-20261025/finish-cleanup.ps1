#requires -Version 7.0
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if (-not (Test-Path -LiteralPath (Join-Path $repo 'native_shell\Cargo.toml'))) {
    throw '未找到本项目源码，停止清理。'
}
$delivery = Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader\最新-20261025'
$installer = Join-Path $delivery 'HLSDownloader-7.0.3-20261025-未签名.msi'
if (-not (Test-Path -LiteralPath $installer) -or
    (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant() -ne '7cc0f1e14ee234bfe6de5e6dce7858b4755b412938de3c2100aac10ac55aebca') {
    throw '桌面成品不存在或哈希不同，停止清理。'
}
if (Get-Process -Name WindowsSandbox,WindowsSandboxClient -ErrorAction SilentlyContinue) {
    throw '请先关闭 Windows 沙盒，避免清理正在使用的映射文件。'
}
if (Get-Process -Name HLSDownloader,HLSDownloaderEngine,HLSDownloaderNativeHost,HLSDownloaderPresenter,HLSDownloaderUpdater -ErrorAction SilentlyContinue) {
    throw '本项目仍在运行，停止清理。'
}
$scratch = Join-Path $repo '.tool-cache\browser-click-20261026'
$relativePaths = @(
    '.tool-cache'
    'desktop_ui\.gradle'
    'desktop_ui\.kotlin'
    'desktop_ui\.tool-cache'
    'desktop_ui\build'
    'desktop_ui\resources\common'
    'extension\node_modules'
    'extension\.output'
    'extension\.wxt'
    'extension\.tool-cache'
    'native_shell\target'
    'presenter_ui\target'
    'scripts\validation\__pycache__'
)
$targets = @()
$sourceLinks = @()
foreach ($relative in $relativePaths) {
    $target = [IO.Path]::GetFullPath((Join-Path $repo $relative))
    if (-not $target.StartsWith($repo + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw '清理目标超出项目，停止。'
    }
    $tracked = @(git -C $repo ls-files -- $relative.Replace('\', '/'))
    if ($LASTEXITCODE -ne 0 -or $tracked.Count -ne 0) {
        throw "目标包含受版本控制的源码，停止清理：$target"
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
        if ($link.LinkType -eq 'SymbolicLink' -and
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
$stateRoot = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'HLSDownloader'
$stateFiles = @()
if (Test-Path -LiteralPath $stateRoot) {
    $stateDirectory = Get-Item -LiteralPath $stateRoot -Force
    if (-not $stateDirectory.PSIsContainer -or ($stateDirectory.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw '本项目窗口状态目录被重定向，停止清理。'
    }
    $stateFiles = @(Get-ChildItem -LiteralPath $stateRoot -Force)
    foreach ($file in $stateFiles) {
        if ($file.PSIsContainer -or $file.Name -notin @('window-geometry.properties', 'workbench.lock', 'workbench.wake') -or
            ($file.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "窗口状态目录存在未确认文件，停止清理：$($file.Name)"
        }
    }
}
foreach ($link in $sourceLinks) { Remove-Item -LiteralPath $link -Force }
foreach ($target in $targets) {
    Remove-Item -LiteralPath $target -Recurse -Force
    Write-Host "已清理：$target"
}
foreach ($file in $stateFiles) {
    Remove-Item -LiteralPath $file.FullName -Force
    Write-Host "已清理本项目窗口状态：$($file.Name)"
}
if ((Test-Path -LiteralPath $stateRoot) -and @(Get-ChildItem -LiteralPath $stateRoot -Force).Count -eq 0) {
    Remove-Item -LiteralPath $stateRoot -Force
}
Write-Host '指定生成目录已清理；源码、Git、桌面成品、用户下载及 outputs/private-user-backup 备份均保留。'
