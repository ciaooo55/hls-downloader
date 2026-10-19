$ErrorActionPreference='Stop'
$work='C:\HLSValidation'
Start-Transcript "$work\guest.log" -Force
try {
    New-Item -ItemType Directory -Force C:\HLSTests\scripts,C:\QAWork | Out-Null
    Copy-Item C:\HLSSource\scripts\validation C:\HLSTests\scripts -Recurse
    Copy-Item "$work\core-tests.exe","C:\HLSBackend\vcruntime140.dll" C:\HLSTests
    Copy-Item C:\HLSSource\assets\app-icon.ico C:\HLSTests
    Copy-Item "$work\compare.py" C:\HLSTests
    Copy-Item C:\HLSPrevious\baseline-image\HLSDownloader C:\HLSRuntime -Recurse
    Copy-Item C:\HLSPrevious\baseline-image\HLSDownloader C:\HLSBaseline -Recurse
    Copy-Item "$work\baseline\*.exe" C:\HLSBaseline\app\resources -Force
    Copy-Item "$work\baseline\*.jar" C:\HLSBaseline\app -Force
    Copy-Item C:\HLSSource\extension\.output C:\HLSTests\extension -Recurse
    $env:TEMP='C:\QAWork';$env:TMP='C:\QAWork';$env:HLS_VM_ACCEPTANCE='1'
    $env:HLS_V6_SKIP_MIGRATE='1';$env:HLS_V7_DATA_DIR='C:\QAWork\unit-data'
    $env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\unit-downloads'
    $env:PYTHONPATH='C:\HLSPrevious\python-packages;C:\HLSTests\scripts\validation'
    $env:PYTHONIOENCODING='utf-8'
    Set-Location C:\HLSTests
    $results=@()
    function Run-Test([string]$Name,[string]$Exe,[string[]]$Arguments,[int]$Timeout=600) {
        $Name | Set-Content "$work\phase.txt"
        $p=Start-Process $Exe -ArgumentList @($Arguments | ForEach-Object {'"'+$_+'"'}) -NoNewWindow -PassThru -RedirectStandardOutput "$work\$Name.log" -RedirectStandardError "$work\$Name.stderr"
        $null=$p.Handle
        if(-not $p.WaitForExit($Timeout*1000)){Stop-Process -Id $p.Id;$code=-1}else{$code=$p.ExitCode}
        $code | Set-Content "$work\$Name.exit"
        $script:results+=@{name=$Name;exit_code=$code}
        $script:results | ConvertTo-Json | Set-Content "$work\results.json"
    }
    Run-Test 'core' 'C:\HLSTests\core-tests.exe' @('--test-threads=1')
    'awaiting-candidate' | Set-Content "$work\phase.txt"
    $deadline=(Get-Date).AddMinutes(10)
    while(-not (Test-Path "$work\candidate.ready")) {
        if((Get-Date) -gt $deadline){throw 'candidate build timed out'}
        Start-Sleep -Seconds 1
    }
    Copy-Item "$work\candidate\*.exe" C:\HLSRuntime\app\resources -Force
    Copy-Item "$work\candidate\*.jar" C:\HLSRuntime\app -Force
    $python='C:\HLSPython\python.exe'
    $scripts='C:\HLSTests\scripts\validation'
    Run-Test 'comparison' $python @('C:\HLSTests\compare.py','--report',"$work\comparison.json") 1200
    $edge='C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
    foreach($capture in @('element','prefetch')) {
        $dom=if($capture -eq 'element'){'dynamic-shadow'}else{'nested-shadow'}
        Run-Test "subtitle-$capture" $python @("$scripts\smoke_extension_subtitles.py",'--extension','C:\HLSTests\extension\chrome-mv3','--host','C:\HLSRuntime\app\resources\HLSDownloaderNativeHost.exe','--browser-binary',$edge,'--capture',$capture,'--player-dom',$dom,'--system-temp','--report',"$work\subtitle-$capture.json")
    }
    Get-Process HLSDownloader,HLSDownloaderEngine -ErrorAction SilentlyContinue | Stop-Process -ErrorAction SilentlyContinue
    $env:HLS_V7_DATA_DIR='C:\QAWork\ui-data';$env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\ui-downloads'
    $engine=Start-Process C:\HLSRuntime\app\resources\HLSDownloaderEngine.exe -PassThru -RedirectStandardOutput "$work\ui-core.log" -RedirectStandardError "$work\ui-core.stderr"
    foreach($stage in @('all','destructive')) {
        Run-Test "workbench-$stage" $python @("$scripts\smoke_v7_workbench_interactions.py",'--runtime','C:\HLSRuntime','--start-at','extras','--extras-stage',$stage,'--report',"$work\workbench-$stage\report.json") 600
    }
    Get-Process HLSDownloader,HLSDownloaderEngine -ErrorAction SilentlyContinue | Stop-Process -ErrorAction SilentlyContinue
    'finished' | Set-Content "$work\finished"
} catch {$_|Out-String|Set-Content "$work\guest.error"} finally {Stop-Transcript}
