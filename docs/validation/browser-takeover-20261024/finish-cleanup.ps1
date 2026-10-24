$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if (-not (Test-Path -LiteralPath (Join-Path $repo 'native_shell\Cargo.toml'))) {
    throw '未找到项目源码，停止清理。'
}
$installer = Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader\最新-20261024\HLSDownloader-7.0.3-20261024-未签名.msi'
if (-not (Test-Path -LiteralPath $installer)) { throw '桌面交付包不存在，停止清理。' }
if ((Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant() -ne '227cabe3c177ff64136c5a1ba10c99b5161eae9a82d8e1194f82d0db4e2aa937') {
    throw '桌面安装包与本轮验收包不同，停止清理。'
}
if (Get-Process -Name WindowsSandbox,WindowsSandboxClient -ErrorAction SilentlyContinue) {
    throw '请先关闭 Windows 沙盒，避免清理正在使用的映射文件。'
}
$relativePaths = @(
    '.tool-cache\browser-takeover-20261024'
    '.tool-cache\interleaved-playback-20261024'
    'extension\node_modules'
    'extension\.output'
    'extension\.wxt'
    'extension\.tool-cache\browser-takeover-20261024'
)
$targets = @()
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
    if (Get-ChildItem -LiteralPath $target -Directory -Recurse -Force | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) {
        throw "目标中存在重解析目录：$target"
    }
    $targets += $target
}
foreach ($target in $targets) {
    Remove-Item -LiteralPath $target -Recurse -Force
    Write-Host "已清理：$target"
}
Write-Host '源码、Git、桌面成品、用户下载和备份均保留。'
