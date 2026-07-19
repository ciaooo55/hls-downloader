param([int]$TimeoutSeconds = 8)

$ErrorActionPreference = "SilentlyContinue"
$configPath = Join-Path $env:LOCALAPPDATA "HLS Downloader\config.json"
$token = "55555"
$port = 8765
if (Test-Path -LiteralPath $configPath) {
    try {
        $configured = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
        if ($configured.token) { $token = [string]$configured.token }
        if ($configured.port) { $port = [int]$configured.port }
    } catch {
    }
}

try {
    Invoke-RestMethod `
        -Method Post `
        -Uri "http://127.0.0.1:$port/api/app/shutdown" `
        -Headers @{ "X-Token" = $token } `
        -ContentType "application/json" `
        -Body "{}" `
        -TimeoutSec 2 | Out-Null
} catch {
}

$deadline = [DateTime]::UtcNow.AddSeconds([Math]::Max(1, $TimeoutSeconds))
do {
    $running = Get-Process HLSDownloader -ErrorAction SilentlyContinue
    if (-not $running) { exit 0 }
    Start-Sleep -Milliseconds 200
} while ([DateTime]::UtcNow -lt $deadline)

exit 1
