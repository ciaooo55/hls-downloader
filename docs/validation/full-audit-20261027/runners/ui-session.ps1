. C:\QARunners\common.ps1
Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith('C:\QAWork\runtime\',[StringComparison]::OrdinalIgnoreCase) } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
$config=Get-Content C:\QAWork\runtime\app\HLSDownloader.cfg
$name=($config | Where-Object {$_ -match '^app.classpath=.*HLSDownloaderDesktop-'}) -replace '^app.classpath=\$APPDIR\\',''
if(@($name).Count -ne 1){throw 'Desktop classpath is not unique'}
Copy-Item C:\QAInput\HLSDownloaderDesktop-7.0.3.jar (Join-Path C:\QAWork\runtime\app $name) -Force
$env:HLS_V7_DATA_DIR='C:\QAWork\ui-data'
$env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\ui-downloads'
Start-Process C:\QAWork\runtime\app\resources\HLSDownloaderEngine.exe -WorkingDirectory C:\QAWork\runtime -WindowStyle Hidden | Out-Null
Run C:\QAWork\browser-python\python.exe @('C:\QARunners\browser-setup.py') 'ui-core-setup'
Start-Process C:\QAWork\browser-python\python.exe -ArgumentList @('C:\QARunners\ui-session.py') -WindowStyle Hidden -RedirectStandardOutput C:\QAReports\ui-seed.stdout.txt -RedirectStandardError C:\QAReports\ui-seed.stderr.txt | Out-Null
