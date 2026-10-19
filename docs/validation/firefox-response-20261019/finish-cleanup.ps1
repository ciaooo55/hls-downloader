[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if ($workspace -ine 'A:\Ubuntu\测试\hls-downloader' -or -not (Test-Path -LiteralPath (Join-Path $workspace '.git'))) {
    throw '项目路径与本轮清理范围不符。'
}
$artifacts = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'desktop-artifacts.json') -Raw | ConvertFrom-Json
foreach ($artifact in $artifacts) {
    if ((Get-FileHash -LiteralPath $artifact.Path -Algorithm SHA256).Hash -ine $artifact.SHA256) {
        throw "桌面交付包与本轮验收哈希不符，停止清理：$($artifact.Path)"
    }
}

# 只列出本轮恢复的依赖、插件生成物和独立验收目录，不匹配其他缓存或用户下载。
foreach ($relative in @('extension\node_modules', 'extension\.output', 'extension\.wxt', '.tool-cache\firefox-response')) {
    $target = [IO.Path]::GetFullPath((Join-Path $workspace $relative))
    if (-not $target.StartsWith($workspace + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw '清理路径越出源码目录。'
    }
    if (Test-Path -LiteralPath $target) {
        if ((Get-Item -LiteralPath $target -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "清理根目录是重解析点，停止处理：$target"
        }
        Remove-Item -LiteralPath $target -Recurse -Force
        Write-Output "已清理：$relative"
    }
}
$cacheRoot = Join-Path $workspace '.tool-cache'
if ((Test-Path -LiteralPath $cacheRoot) -and @(Get-ChildItem -LiteralPath $cacheRoot -Force).Count -eq 0) {
    Remove-Item -LiteralPath $cacheRoot -Force
}
Write-Output '完成：源码、Git、四份桌面成品、用户下载及其他项目均保留。'
