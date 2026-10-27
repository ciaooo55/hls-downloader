. C:\QARunners\common.ps1
Copy-Item C:\QAPerf\runtime C:\QAWork\runtime -Recurse
foreach ($name in @('HLSDownloaderEngine.exe','HLSDownloaderNativeHost.exe','HLSDownloaderUpdater.exe')) {
    Copy-Item (Join-Path C:\QAInput $name) C:\QAWork\runtime\app\resources -Force
}
Copy-Item C:\QAInput\HLSDownloaderDesktop-7.0.3.jar C:\QAWork\runtime\app -Force
$python='C:\QAPerf\python\python.exe'
$engine='C:\QAWork\runtime\app\resources\HLSDownloaderEngine.exe'
Run $python @('C:\QAScripts\smoke_v7_task_import.py','--engine',$engine,'--report','C:\QAReports\final-task-import.json') 'final-task-import'
Run $python @('C:\QAScripts\smoke_v7_http_stall.py','--engine',$engine,'--root','C:\QAWork\stall','--report','C:\QAReports\http-stall.json') 'http-stall'
Run $python @('C:\QAScripts\smoke_v7_download_playback.py','--runtime','C:\QAWork\runtime','--report','C:\QAReports\download-playback.json') 'download-playback'
