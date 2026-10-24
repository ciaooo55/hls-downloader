[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..')).TrimEnd('\')
$target = [IO.Path]::GetFullPath((Join-Path $repo '.tool-cache\player-software-20261024'))
if ($target -ne "$repo\.tool-cache\player-software-20261024") { throw 'Unexpected cleanup target.' }
if (-not (Test-Path -LiteralPath (Join-Path $repo '.git'))) { throw 'Repository identity missing.' }
if (Test-Path -LiteralPath $target) {
    $item = Get-Item -LiteralPath $target -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing a redirected cleanup target.' }
    Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction Stop
}
$copies = @(
    'results\1080-isolation\correct-downscaling-no.stderr.log',
    'results\1080-isolation\correct-downscaling-no.stdout.log',
    'results\1080-isolation\dither-no.stderr.log',
    'results\1080-isolation\dither-no.stdout.log',
    'results\1080-isolation\fast-profile.stderr.log',
    'results\1080-isolation\fast-profile.stdout.log',
    'results\1080-isolation\generate-1080.stderr.log',
    'results\1080-isolation\generate-1080.stdout.log',
    'results\gpu\installer.stderr.log',
    'results\gpu\installer.stdout.log'
)
foreach ($relative in $copies) {
    $path = Join-Path $PSScriptRoot $relative
    if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force -ErrorAction Stop }
}
Write-Output '本轮播放器构建缓存、依赖副本与沙盒临时文件已清理；源码、Git、桌面成品、备份及用户下载均保留。'
