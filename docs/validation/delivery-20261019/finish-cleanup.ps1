# 自动审批拒绝了代理删除操作；此脚本仅供用户审阅后手动执行。
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../../..')).Path
$delivery = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'desktop-artifacts.json') -Raw | ConvertFrom-Json
foreach ($artifact in $delivery) {
    if ((Get-FileHash -LiteralPath $artifact.Path -Algorithm SHA256).Hash -ne $artifact.SHA256) {
        throw "桌面交付包缺失或发生变化，停止清理：$($artifact.Path)"
    }
}
$targets = @(
    '.tool-cache', 'artifacts', 'desktop_ui/.gradle', 'desktop_ui/.kotlin',
    'desktop_ui/.tool-cache', 'desktop_ui/build',
    'extension/.output', 'extension/.wxt', 'extension/node_modules'
)
foreach ($relative in $targets) {
    $path = [IO.Path]::GetFullPath((Join-Path $repo $relative))
    if (-not $path.StartsWith($repo + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "清理路径超出项目：$path"
    }
    $tracked = @(git -C $repo ls-files -- $relative)
    if ($LASTEXITCODE -ne 0 -or $tracked.Count -ne 0) {
        throw "不能确认目录不含受版本控制的源码，停止清理：$path"
    }
}
foreach ($relative in $targets) {
    $path = Join-Path $repo $relative
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction Stop
        Write-Output "已清理：$path"
    }
}
Write-Output '完成：仅清理项目内列明的构建目录；源码、Git、桌面交付包及用户下载均保留。'
