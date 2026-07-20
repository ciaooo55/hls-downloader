param(
    [int]$TimeoutSeconds = 20,
    [string]$InstallDir = ""
)

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

$gracefulDeadline = [DateTime]::UtcNow.AddSeconds([Math]::Min(8, [Math]::Max(1, $TimeoutSeconds)))
while ([DateTime]::UtcNow -lt $gracefulDeadline) {
    if (-not (Get-Process HLSDownloader -ErrorAction SilentlyContinue)) { break }
    Start-Sleep -Milliseconds 200
}

if (Get-Process HLSDownloader -ErrorAction SilentlyContinue) {
    & "$env:SystemRoot\System32\taskkill.exe" /IM HLSDownloader.exe /T /F | Out-Null
    Get-Process HLSDownloader -ErrorAction SilentlyContinue | Stop-Process -Force
}

function Test-ExecutableWritable {
    if (-not $InstallDir) { return $true }
    $target = Join-Path $InstallDir "HLSDownloader.exe"
    if (-not (Test-Path -LiteralPath $target)) { return $true }
    try {
        $stream = [System.IO.File]::Open(
            $target,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::None
        )
        $stream.Dispose()
        return $true
    } catch {
        return $false
    }
}

$deadline = [DateTime]::UtcNow.AddSeconds([Math]::Max(3, $TimeoutSeconds))
do {
    $running = Get-Process HLSDownloader -ErrorAction SilentlyContinue
    if (-not $running -and (Test-ExecutableWritable)) { exit 0 }
    if ($running) {
        $running | Stop-Process -Force
    }
    Start-Sleep -Milliseconds 250
} while ([DateTime]::UtcNow -lt $deadline)

exit 1
