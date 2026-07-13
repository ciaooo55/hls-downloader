# Build frontend for production
Write-Output "Building frontend..."
$pnpm = Get-Command pnpm -ErrorAction SilentlyContinue
if (-not $pnpm) {
    Write-Error "pnpm not found. Install pnpm first if you need to rebuild the frontend."
    exit 1
}
Set-Location "$PSScriptRoot\frontend"
if (-not (Test-Path "node_modules")) {
    pnpm install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
pnpm run build
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Output "Build complete!"
