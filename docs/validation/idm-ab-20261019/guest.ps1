$ErrorActionPreference='Stop'
$work='C:\HLSValidation\idm-ab-20261019'
Start-Transcript "$work\guest.log" -Force
try {
    New-Item -ItemType Directory -Force C:\HLSTests\scripts,C:\QAWork | Out-Null
    Copy-Item C:\HLSSource\scripts\validation C:\HLSTests\scripts -Recurse
    Copy-Item "$work\core-tests.exe",C:\HLSBackend\vcruntime140.dll C:\HLSTests
    Copy-Item C:\HLSSource\assets\app-icon.ico C:\HLSTests
    Copy-Item C:\HLSPrevious\baseline-image\HLSDownloader C:\HLSRuntime -Recurse
    Copy-Item C:\HLSValidation\single-task\candidate\*.jar C:\HLSRuntime\app -Force
    Copy-Item "$work\candidate\*.exe" C:\HLSRuntime\app\resources -Force
    Copy-Item "$work\ab\ABDownloadManager" C:\ABDownloadManager -Recurse
    $env:TEMP='C:\QAWork';$env:TMP='C:\QAWork';$env:HLS_VM_ACCEPTANCE='1'
    $env:HLS_V6_SKIP_MIGRATE='1';$env:HLS_V7_DATA_DIR='C:\QAWork\unit-data'
    $env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\unit-downloads'
    $env:PYTHONPATH='C:\HLSPrevious\python-packages;C:\HLSTests\scripts\validation'
    $env:PYTHONIOENCODING='utf-8'
    Set-Location C:\HLSTests
    $results=@()
    function Run-Test([string]$Name,[string]$Exe,[string[]]$Arguments,[int]$Timeout=900) {
        $Name | Set-Content "$work\phase.txt"
        $p=Start-Process $Exe -ArgumentList @($Arguments | ForEach-Object {'"'+$_+'"'}) -NoNewWindow -PassThru -RedirectStandardOutput "$work\$Name.log" -RedirectStandardError "$work\$Name.stderr"
        $null=$p.Handle
        if(-not $p.WaitForExit($Timeout*1000)){Stop-Process -Id $p.Id;$code=-1}else{$code=$p.ExitCode}
        $code | Set-Content "$work\$Name.exit"
        $script:results+=@{name=$Name;exit_code=$code}
        $script:results | ConvertTo-Json | Set-Content "$work\results.json"
    }
    Run-Test 'core' 'C:\HLSTests\core-tests.exe' @('--test-threads=1')
    $python='C:\HLSPython\python.exe'
    $scripts='C:\HLSTests\scripts\validation'
    $engine='C:\HLSRuntime\app\resources\HLSDownloaderEngine.exe'
    Run-Test 'download-playback' $python @("$scripts\smoke_v7_download_playback.py",'--runtime','C:\HLSRuntime','--report',"$work\download-playback.json")
    Run-Test 'http-boundaries' $python @("$scripts\smoke_v7_http_boundaries.py",'--engine',$engine,'--report',"$work\http-boundaries.json")
    Run-Test 'http-stall' $python @("$scripts\smoke_v7_http_stall.py",'--engine',$engine,'--root','C:\QAWork\http-stall','--report',"$work\http-stall.json")
    Run-Test 'idm-install' "$work\idman643build15.exe" @('/skipdlgs') 180
    'ready' | Set-Content "$work\guest.ready"
    $deadline=(Get-Date).AddHours(2)
    while((Get-Date) -lt $deadline -and -not(Test-Path "$work\stop")) {
        foreach($job in Get-ChildItem "$work\queue" -Filter '*.ps1' -ErrorAction SilentlyContinue | Sort-Object Name) {
            if(Test-Path ($job.FullName+'.exit')){continue}
            try {& $job.FullName *> ($job.FullName+'.log'); '0' | Set-Content ($job.FullName+'.exit')}
            catch {$_ | Out-String | Add-Content ($job.FullName+'.log'); '1' | Set-Content ($job.FullName+'.exit')}
        }
        Start-Sleep -Milliseconds 500
    }
} catch {$_|Out-String|Set-Content "$work\guest.error"} finally {Stop-Transcript}
