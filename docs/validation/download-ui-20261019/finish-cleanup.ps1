#requires -Version 7.0
$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../../..')).Path
if ($projectRoot -ne 'A:\Ubuntu\测试\hls-downloader') { throw '项目位置不匹配，未执行清理。' }

$desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader'
$delivery = Join-Path $desktop '最新-46d09b5'
$evidence = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'result.json') -Raw | ConvertFrom-Json
foreach ($file in $evidence.delivery_files) {
    $path = Join-Path $delivery $file.name
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256) {
        throw "新交付包哈希不匹配，未执行清理：$path"
    }
}

# 仅关闭本轮创建的沙盒；PID 或启动时间变化时保留其他沙盒。
$sandbox = Get-Process -Id 20092 -ErrorAction SilentlyContinue
$createdAt = [datetime]'2026-10-19T17:09:44.5155901+08:00'
if ($sandbox -and $sandbox.ProcessName -eq 'WindowsSandboxClient' -and
    $sandbox.Path -eq 'C:\Windows\System32\WindowsSandboxClient.exe' -and
    [Math]::Abs(($sandbox.StartTime - $createdAt).TotalSeconds) -lt 1) {
    Stop-Process -Id $sandbox.Id
    $sandbox.WaitForExit(10000) | Out-Null
}

foreach ($relative in @('.tool-cache', 'desktop_ui\.gradle', 'desktop_ui\.kotlin',
    'desktop_ui\.tool-cache', 'desktop_ui\build', 'extension\node_modules', 'extension\.wxt', 'extension\.output')) {
    $path = [IO.Path]::GetFullPath((Join-Path $projectRoot $relative))
    if (-not $path.StartsWith($projectRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw '清理路径超出项目，已停止。'
    }
    if (Test-Path -LiteralPath $path) {
        if ((Get-Item -LiteralPath $path -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "目标是重解析点，保留待检查：$path"
        }
        Remove-Item -LiteralPath $path -Recurse -Force
        Write-Host "已清理项目生成目录：$relative"
    }
}

$oldPackages = @{
    'HLSDownloader-7.0.3-Chromium.zip' = '95a93ecbd5e6587c847dbd8752e5eebd4ef5bf233d14333c871663045055c723'
    'HLSDownloader-7.0.3-Firefox-未签名.zip' = '6723dde3ec66b9ce818d8594958942104079fcc70cde1d389e83d5b63269334e'
    'HLSDownloader-7.0.3-main-7c42c86-未签名.msi' = '8da9e4073075afaa7a176d75736262f01c6bf22681908ccea5a9050a21db8e8d'
}
foreach ($entry in $oldPackages.GetEnumerator()) {
    $path = Join-Path $desktop $entry.Key
    if (-not (Test-Path -LiteralPath $path)) { continue }
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $entry.Value) {
        Write-Warning "旧包内容已变化，保留：$path"
        continue
    }
    Remove-Item -LiteralPath $path -Force
    Write-Host "已清理旧交付包：$($entry.Key)"
}
Write-Host '收尾完成。源码、Git、新交付包、本机已有程序及用户下载均保留。'
