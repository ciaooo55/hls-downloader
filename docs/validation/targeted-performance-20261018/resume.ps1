$ErrorActionPreference='Stop'
$work='C:\HLSValidation'
$env:HLS_VM_ACCEPTANCE='1';$env:HLS_V6_SKIP_MIGRATE='1'
$env:TEMP='C:\QAWork';$env:TMP='C:\QAWork'
$env:HLS_V7_DATA_DIR='C:\QAWork\ui-data';$env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\ui-downloads'
$env:PYTHONPATH='C:\HLSPrevious\python-packages;C:\HLSTests\scripts\validation'
$env:PYTHONIOENCODING='utf-8'
Set-Location C:\HLSTests
$engine=Start-Process C:\HLSRuntime\app\resources\HLSDownloaderEngine.exe -PassThru -RedirectStandardOutput "$work\rerun-core.log" -RedirectStandardError "$work\rerun-core.stderr"
$results=@()
foreach($stage in @('all','destructive')) {
    $name="rerun-workbench-$stage"
    $name | Set-Content "$work\phase.txt"
    $arguments=@('C:\HLSTests\scripts\validation\smoke_v7_workbench_interactions.py','--runtime','C:\HLSRuntime','--start-at','extras','--extras-stage',$stage,'--report',"$work\$name\report.json")
    $p=Start-Process C:\HLSPython\python.exe -ArgumentList @($arguments|ForEach-Object {'"'+$_+'"'}) -NoNewWindow -PassThru -RedirectStandardOutput "$work\$name.log" -RedirectStandardError "$work\$name.stderr"
    $null=$p.Handle
    if(-not $p.WaitForExit(600000)){Stop-Process -Id $p.Id;$code=-1}else{$code=$p.ExitCode}
    $results+=@{name=$name;exit_code=$code}
    $results|ConvertTo-Json|Set-Content "$work\rerun-results.json"
    if($code -ne 0){break}
}
Get-Process HLSDownloader,HLSDownloaderEngine -ErrorAction SilentlyContinue | Stop-Process -ErrorAction SilentlyContinue
'finished'|Set-Content "$work\rerun.finished"
