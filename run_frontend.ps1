Write-Output "Starting HLS Downloader frontend (dev mode)..."
Set-Location "$PSScriptRoot\frontend"
$vite = Join-Path (Get-Location) "node_modules\.bin\vite.cmd"
if (Test-Path $vite) {
    & $vite
} else {
    Write-Error "Local frontend dependencies not found. Run: cd frontend; pnpm install"
    exit 1
}
