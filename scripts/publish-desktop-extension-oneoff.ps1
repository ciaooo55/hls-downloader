# 一次性发布脚本:桌面每浏览器只保留一份当前版本扩展包(更新先删旧)。
# 命名与 scripts/install-v7-local.ps1 的桌面发布约定一致。
$ErrorActionPreference = 'Stop'
$desktop = [Environment]::GetFolderPath('Desktop')
$output = Join-Path $PSScriptRoot '..\extension\.output'

# 删除桌面上的全部旧扩展包(规范名 + 安装脚本历史名 + wxt 版本名)
$stale = Get-ChildItem -LiteralPath $desktop -File -Filter '*.zip' -ErrorAction SilentlyContinue |
    Where-Object {
        ($_.Name -match '^(HLS ?Downloader|hls-downloader-extension)') -and
        ($_.Name -match '(?i)(chromium|chrome|firefox)')
    }
$stale | ForEach-Object {
    Write-Output ("removed old: " + $_.Name)
    Remove-Item -LiteralPath $_.FullName -Force
}

$copies = @{
    'HLSDownloader-Chromium.zip' = 'hls-downloader-extension-7.0.0-chrome.zip'
    'HLSDownloader-Firefox.zip'  = 'hls-downloader-extension-7.0.0-firefox.zip'
}
foreach ($entry in $copies.GetEnumerator()) {
    Copy-Item -LiteralPath (Join-Path $output $entry.Value) -Destination (Join-Path $desktop $entry.Key) -Force
}

$published = Get-ChildItem -LiteralPath $desktop -File -Filter '*.zip' |
    Where-Object { $_.Name -match '(?i)(chromium|chrome|firefox)' }
$published | Select-Object Name, Length, LastWriteTime | Format-Table -AutoSize
if ($published.Count -ne 2) { throw "expected exactly 2 extension packages, got $($published.Count)" }
Write-Output 'desktop extension publish OK'
