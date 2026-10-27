$ErrorActionPreference = 'Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
$env:HLS_VM_ACCEPTANCE = '1'
$env:PYTHONDONTWRITEBYTECODE = '1'
$env:TEMP = 'C:\QAWork\tmp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force C:\QAWork,$env:TEMP | Out-Null
Set-Location C:\QAWork
$result = [ordered]@{ finished=$false; passed=$false; checks=@(); current='before' }
function Save { $result | ConvertTo-Json -Depth 8 | Set-Content C:\QAReports\summary.json -Encoding UTF8 }
function Run([string]$exe, [string[]]$arguments, [string]$name, [bool]$expectFailure=$false) {
    $result.current=$name; Save
    $p=Start-Process $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "C:\QAReports\$name.stdout.txt" -RedirectStandardError "C:\QAReports\$name.stderr.txt"
    [void]$p.Handle
    if (-not $p.WaitForExit(600000)) { Stop-Process -Id $p.Id -Force; throw "$name timeout" }
    $result.checks+=@{name=$name; exit_code=$p.ExitCode; expected_failure=$expectFailure}; Save
    if (($expectFailure -and $p.ExitCode -eq 0) -or (-not $expectFailure -and $p.ExitCode -ne 0)) { throw "$name unexpected exit $($p.ExitCode)" }
}
try {
    Save
    Copy-Item C:\QAInput\before-tests.exe,C:\QAPerf\throttle\vcruntime140.dll C:\QAWork
    Copy-Item C:\QAPerf\runtime\app\resources\app-icon.ico C:\QAWork
    Run C:\QAWork\before-tests.exe @('task_export::tests::','--test-threads=1') 'before-export' $true
    Run C:\QAWork\before-tests.exe @('download_worker::tests::import_paths_continues_after_a_bad_file_and_reports_partial_success','--exact') 'before-import' $true
    $result.current='await-candidate'; Save
    $deadline=[DateTime]::UtcNow.AddMinutes(15)
    while (-not (Test-Path C:\QAInput\candidate.ready)) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'candidate not provided' }
        Start-Sleep -Seconds 2
    }
    Copy-Item C:\QAInput\after-tests.exe C:\QAWork
    $env:HLS_FFMPEG='C:\QAPerf\runtime\app\resources\ffmpeg.exe'
    $env:HLS_V7_CURL_IMPERSONATE='C:\QAPerf\runtime\app\resources\tools\curl-impersonate\curl-impersonate.exe'
    $env:HLS_V7_LIBMPV='C:\QAPerf\runtime\app\resources\libmpv-2.dll'
    Run C:\QAWork\after-tests.exe @('--test-threads=1') 'core-regression'
    Run C:\QAPerf\python\python.exe @('C:\QAScripts\smoke_v7_task_import.py','--engine','C:\QAInput\HLSDownloaderEngine.exe','--report','C:\QAReports\task-import.json') 'task-import'
    Run C:\QAPerf\python\python.exe @('C:\QAScripts\smoke_v7_http_boundaries.py','--engine','C:\QAInput\HLSDownloaderEngine.exe','--report','C:\QAReports\http-boundaries.json') 'http-boundaries'
    $result.passed=$true
} catch { $result.error=$_.Exception.Message; $result.position=$_.InvocationInfo.PositionMessage }
finally { $result.finished=$true; Save }
