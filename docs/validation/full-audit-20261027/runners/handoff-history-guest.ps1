$ErrorActionPreference='Stop'
if($env:USERNAME -ne 'WDAGUtilityAccount'){throw 'Windows Sandbox only'}
New-Item -ItemType Directory -Force C:\QAWork,C:\QAWork\tmp,C:\QAWork\before,C:\QAWork\after | Out-Null
$env:TEMP='C:\QAWork\tmp';$env:TMP=$env:TEMP
. C:\QARunners\common.ps1
Copy-Item C:\QAPerf\python C:\QAWork\python -Recurse
"python312.zip`n.`nC:\QAScripts" | Set-Content C:\QAWork\python\python312._pth -Encoding ASCII
Copy-Item C:\QABaseline\HLSDownloaderEngine.exe,C:\QABaseline\HLSDownloaderNativeHost.exe,C:\QAPerf\throttle\vcruntime140.dll C:\QAWork\before
try {
    Run C:\QAWork\python\python.exe @('C:\QAScripts\smoke_v7_handoff_history.py','--runtime','C:\QAWork\before','--report','C:\QAReports\before-history.json','--expect-legacy-failure') 'before-history'
    'finished' | Set-Content C:\QAReports\before-finished.txt
    $deadline=[DateTime]::UtcNow.AddMinutes(60)
    while(-not (Test-Path C:\QAInput\fixed.ready)){
        if([DateTime]::UtcNow -gt $deadline){throw 'Fixed artifacts were not provided'}
        Start-Sleep -Seconds 2
    }
    Copy-Item C:\QAInput\HLSDownloaderEngine.exe,C:\QAInput\HLSDownloaderNativeHost.exe,C:\QAPerf\throttle\vcruntime140.dll C:\QAWork\after
    Copy-Item C:\QAInput\tests.exe,C:\QAPerf\throttle\vcruntime140.dll,C:\QAPerf\runtime\app\resources\app-icon.ico C:\QAWork
    foreach($module in @('native_host','store','core_server','core_ipc')){
        Run C:\QAWork\tests.exe @("${module}::tests::",'--test-threads=1') "tests-$module"
    }
    Run C:\QAWork\python\python.exe @('C:\QAScripts\smoke_v7_handoff_history.py','--runtime','C:\QAWork\after','--report','C:\QAReports\after-history.json') 'after-history'
} catch { $_ | Out-String | Set-Content C:\QAReports\handoff-history-error.txt; throw }
finally { 'finished' | Set-Content C:\QAReports\handoff-history-finished.txt }
