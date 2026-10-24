$ErrorActionPreference = 'Stop'
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if (-not (Test-Path -LiteralPath (Join-Path $repoRoot 'native_shell\Cargo.toml'))) {
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
$target = [IO.Path]::GetFullPath((Join-Path $repoRoot '.tool-cache\interleaved-playback-20261024'))
if ($target -ne (Join-Path $repoRoot '.tool-cache\interleaved-playback-20261024')) { throw '清理目标校验失败。' }
if (Test-Path -LiteralPath $target) {
    $directories = @((Get-Item -LiteralPath $target)) + @(Get-ChildItem -LiteralPath $target -Directory -Recurse -Force)
    if ($directories | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }) {
        throw '发现重解析目录，停止清理以保护其他位置。'
    }
    Remove-Item -LiteralPath $target -Recurse -Force
}
Write-Host '已清理本轮列明的构建目录；源码、Git、桌面成品、用户下载及备份均保留。'
