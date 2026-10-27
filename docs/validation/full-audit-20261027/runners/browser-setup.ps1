. C:\QARunners\common.ps1
Copy-Item C:\QAPerf\python C:\QAWork\browser-python -Recurse
"python312.zip`n.`nC:\QAInput\pydeps`nC:\QAPerf\pydeps`nC:\QAScripts" | Set-Content C:\QAWork\browser-python\python312._pth -Encoding ASCII
New-Item -ItemType File C:\QAWork\runtime\portable -Force | Out-Null
$env:HLS_V7_DATA_DIR='C:\QAWork\browser-data'
$env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\browser-downloads'
Start-Process C:\QAWork\runtime\app\resources\HLSDownloaderEngine.exe -WorkingDirectory C:\QAWork\runtime -WindowStyle Hidden | Out-Null
Run C:\QAWork\browser-python\python.exe @('C:\QARunners\browser-setup.py') 'browser-setup'
