$work='C:\HLSValidation\idm-ab-20261019'
Copy-Item "$work\firefox-slow-start.json" "$work\firefox-alt-native-headless-error.json" -Force
Get-Process | Where-Object {$_.Path -like 'C:\HLSRuntime\*'} | Stop-Process -Force
Copy-Item C:\HLSSource\extension\.output\firefox-mv3\* C:\HLSExtension\firefox-mv3 -Recurse -Force
$env:HLS_V7_DATA_DIR='C:\QAWork\firefox-final-owned-data'
$env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\firefox-final-owned-output'
$p=Start-Process C:\HLSPython\python.exe -ArgumentList ('"'+$work+'\firefox-slow-start.py"') -NoNewWindow -PassThru -RedirectStandardOutput "$work\firefox-owned-final.log" -RedirectStandardError "$work\firefox-owned-final.stderr"
$null=$p.Handle
if(-not $p.WaitForExit(240000)){Stop-Process -Id $p.Id;throw 'Firefox ownership fixture timeout'}
$report=Get-Content "$work\firefox-slow-start.json" -Raw|ConvertFrom-Json
if(-not $report.passed){throw 'Firefox ownership validation did not pass'}
