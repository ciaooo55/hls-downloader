# 自动审批拒绝代理删除本轮缓存；供用户检查后手动执行。
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../../..')).Path
$artifacts = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'desktop-artifacts.json') -Raw | ConvertFrom-Json
foreach ($artifact in $artifacts) {
    if ((Get-FileHash -LiteralPath $artifact.Path -Algorithm SHA256).Hash -ne $artifact.SHA256) {
        throw "桌面交付包缺失或变化，停止清理：$($artifact.Path)"
    }
}
$target = [IO.Path]::GetFullPath((Join-Path $repo '.tool-cache/installer-upgrade'))
if (-not $target.StartsWith($repo + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw '清理目标超出项目目录'
}
if (Test-Path -LiteralPath $target) {
    if ((Get-Item -LiteralPath $target).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw '缓存目录是重解析点，停止清理'
    }
    $tracked = @(git -C $repo ls-files -- '.tool-cache/installer-upgrade')
    if ($LASTEXITCODE -ne 0 -or $tracked.Count -ne 0) { throw '不能确认目标不含受版本控制的源码' }
    Remove-Item -LiteralPath $target -Recurse -Force
}
$cache = Join-Path $repo '.tool-cache'
if ((Test-Path -LiteralPath $cache) -and @(Get-ChildItem -LiteralPath $cache -Force).Count -eq 0) {
    Remove-Item -LiteralPath $cache -Force
}
Write-Output '本轮安装器缓存已清理；源码、Git、验收记录、桌面成品及用户下载均保留。'
