[CmdletBinding(SupportsShouldProcess)]
param()

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (-not (Test-Path -LiteralPath (Join-Path $repo 'native_shell/src/player.rs')) -or
    -not (Test-Path -LiteralPath (Join-Path $repo 'desktop_ui/src/main/kotlin/com/hlsdownloader/desktop/VideoPlayer.kt'))) {
    throw '当前脚本不在预期的 HLS Downloader 源码目录，未执行清理。'
}

$desktop = [Environment]::GetFolderPath('Desktop')
$latest = Join-Path $desktop 'HLSDownloader-7.0.3-r6-未签名.msi'
$latestHash = '5f78da410fcf0c55dfc8a9cd7d470b50a3b27455cd68ee0f9f30093bfc97a96a'
$oldPackages = @{
    'HLSDownloader-7.0.3-r5-未签名.msi' = '0536637a1f65aad034907ba23bd68df3e6017ed8ccb123b7c570f3b125d90393'
    'HLSDownloader-7.0.3-cache-playback-未签名.msi' = 'dca68bc346f6b262c4b2887e21f10cec37f9b53e347e98b0d6fab092bdeb971a'
    'HLSDownloader-7.0.3-v3-player-未签名.msi' = '96158caea3914bb3857703efd34bf48798545a629735e67057abde1d79fe6b5c'
}
if (-not (Test-Path -LiteralPath $latest) -or
    (Get-FileHash -LiteralPath $latest -Algorithm SHA256).Hash -ine $latestHash) {
    throw '桌面最新验收包不存在或已改变，停止清理以保留现有交付。'
}
foreach ($name in $oldPackages.Keys) {
    $old = Join-Path $desktop $name
    if ((Test-Path -LiteralPath $old) -and
        (Get-FileHash -LiteralPath $old -Algorithm SHA256).Hash -ine $oldPackages[$name]) {
        throw "旧安装包内容已改变，停止避免误删：$name"
    }
}

# 仅本轮生成目录；不遍历下载目录、用户备份、注册表或其他项目。
$targets = @('.tool-cache/build-cache', '.tool-cache/player-v3-validation',
             'desktop_ui/.gradle', 'desktop_ui/.kotlin',
             'extension/node_modules', 'extension/.output', 'extension/.wxt')
foreach ($relative in $targets) {
    $path = [IO.Path]::GetFullPath((Join-Path $repo $relative))
    if (-not $path.StartsWith($repo + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "清理目录越界：$relative"
    }
    if (-not (Test-Path -LiteralPath $path)) { continue }
    if ((Get-Item -LiteralPath $path -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "生成目录是重解析点，停止：$relative"
    }
    if ($PSCmdlet.ShouldProcess($path, '删除本轮构建或沙盒验收生成目录')) {
        Remove-Item -LiteralPath $path -Recurse -Force
        Write-Output "已清理：$relative"
    }
}
foreach ($name in $oldPackages.Keys) {
    $old = Join-Path $desktop $name
    if ((Test-Path -LiteralPath $old) -and $PSCmdlet.ShouldProcess($old, '删除哈希匹配的旧安装包')) {
        Remove-Item -LiteralPath $old -Force
        Write-Output "已清理旧安装包：$name"
    }
}
$cacheRoot = Join-Path $repo '.tool-cache'
if ((Test-Path -LiteralPath $cacheRoot) -and
    @(Get-ChildItem -LiteralPath $cacheRoot -Force).Count -eq 0 -and
    $PSCmdlet.ShouldProcess($cacheRoot, '删除空缓存容器')) {
    Remove-Item -LiteralPath $cacheRoot -Force
}
