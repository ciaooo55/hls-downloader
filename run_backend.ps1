Write-Output "Starting HLS Downloader backend..."
$env:PATH = "$PSScriptRoot\bin;$env:PATH"
Set-Location "$PSScriptRoot\backend"
python -m uvicorn app.main:app --host 127.0.0.1 --port 8765
