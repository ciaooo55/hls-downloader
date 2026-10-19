$work='C:\HLSValidation\idm-ab-20261019'
Copy-Item "$work\firefox-slow-start.json" "$work\firefox-final.json" -Force
Get-Process | Where-Object {$_.Path -like 'C:\HLSRuntime\*'} | Stop-Process -Force
$env:HLS_V7_DATA_DIR='C:\QAWork\firefox-ui-data'
$env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\firefox-ui-output'
$p=Start-Process C:\HLSPython\python.exe -ArgumentList @(('"'+$work+'\firefox-slow-start.py"'),'--ui-decisions') -NoNewWindow -PassThru -RedirectStandardOutput "$work\firefox-ui.log" -RedirectStandardError "$work\firefox-ui.stderr"
$null=$p.Handle
if(-not $p.WaitForExit(240000)){Stop-Process -Id $p.Id;throw 'Firefox UI fixture timeout'}
Copy-Item "$work\firefox-slow-start.json" "$work\firefox-ui-final.json" -Force
$report=Get-Content "$work\firefox-ui-final.json" -Raw|ConvertFrom-Json
if(-not $report.passed){throw 'Firefox UI decisions did not pass'}
