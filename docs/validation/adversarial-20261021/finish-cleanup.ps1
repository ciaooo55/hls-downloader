[CmdletBinding(SupportsShouldProcess)]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../../..')).Path
$desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader'
$delivery = Get-Content (Join-Path $PSScriptRoot 'delivery.json') -Raw | ConvertFrom-Json
$latest = Join-Path $desktop $delivery.directory
foreach ($asset in $delivery.assets) {
    $path = Join-Path $latest $asset.name
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $asset.sha256) {
        throw "桌面成品缺失或哈希不符，停止清理：$path"
    }
}
if (Get-Process WindowsSandboxClient -ErrorAction SilentlyContinue) {
    throw '请先关闭 Windows 沙盒，避免清理仍被测试使用的映射目录。'
}

$targets = @(
    '.tool-cache', 'artifacts', 'desktop_ui/.gradle', 'desktop_ui/.kotlin',
    'desktop_ui/.tool-cache', 'desktop_ui/build', 'desktop_ui/resources/common',
    'extension/.output', 'extension/.wxt', 'extension/node_modules'
)
foreach ($relative in $targets) {
    $path = [IO.Path]::GetFullPath((Join-Path $repo $relative))
    if (-not $path.StartsWith($repo + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "清理目标超出项目：$path"
    }
    $tracked = @(git -C $repo ls-files -- $relative)
    if ($LASTEXITCODE -ne 0 -or $tracked.Count) {
        throw "清理目标含受版本控制文件或无法检查：$relative"
    }
    if ((Test-Path -LiteralPath $path) -and ((Get-Item -LiteralPath $path -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "清理目标是链接，停止清理：$path"
    }
}

$previous = Get-Content (Join-Path $PSScriptRoot 'old-delivery.json') -Raw | ConvertFrom-Json
$old = [IO.Path]::GetFullPath((Join-Path $desktop $previous.directory))
if ([IO.Path]::GetDirectoryName($old) -ne [IO.Path]::GetFullPath($desktop) -or $old -eq $latest) {
    throw '旧交付目录不符合白名单边界。'
}
if (Test-Path -LiteralPath $old) {
    if ((Get-Item -LiteralPath $old -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw '旧交付目录是链接，停止清理。'
    }
    $files = @(Get-ChildItem -LiteralPath $old -Force)
    if ($files.Count -ne $previous.files.Count) { throw '旧交付目录内容已变化，保留。' }
    foreach ($file in $files) {
        $expected = @($previous.files | Where-Object name -eq $file.Name)
        if ($file.PSIsContainer -or $expected.Count -ne 1 -or
            ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
            (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash -ne $expected[0].sha256) {
            throw "旧交付文件已变化，保留：$($file.FullName)"
        }
    }
}

# 所有边界、源码与成品检查完成后才开始删除；用户任务、密钥和下载备份不在清单内。
foreach ($relative in $targets) {
    $path = Join-Path $repo $relative
    if ((Test-Path -LiteralPath $path) -and $PSCmdlet.ShouldProcess($path, '删除本项目可重建生成目录')) {
        Remove-Item -LiteralPath $path -Recurse -Force
        Write-Output "已清理：$path"
    }
}
if ((Test-Path -LiteralPath $old) -and $PSCmdlet.ShouldProcess($old, '删除已核对哈希的旧交付包')) {
    Remove-Item -LiteralPath $old -Recurse -Force
}
if ($WhatIfPreference) {
    Write-Output '仅预览；未删除任何文件。'
} else {
    Write-Output '完成；源码、Git、最新桌面成品、用户下载和 outputs/private-user-backup 均保留。'
}
