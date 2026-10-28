$ErrorActionPreference='Stop'
if($env:USERNAME -ne 'WDAGUtilityAccount'){throw 'Windows Sandbox only'}
New-Item -ItemType Directory -Force C:\QAWork,C:\QAWork\tmp | Out-Null
$env:TEMP='C:\QAWork\tmp';$env:TMP=$env:TEMP
. C:\QARunners\common.ps1
Copy-Item C:\QAInput\tests.exe,C:\QAPerf\throttle\vcruntime140.dll,C:\QAPerf\runtime\app\resources\app-icon.ico C:\QAWork
try {
    foreach($filter in @('core_service::tests::','download_worker::tests::browser_media_push','download_worker::tests::expired_presenter_lease','download_worker::tests::presenter_lease','download_worker::tests::rejected_handoff','native_host::tests::native_host_connection_and_handoff_io','store::tests::handoff_queries')){
        $name='tests-'+($filter -replace '::','-').TrimEnd('-')
        Run C:\QAWork\tests.exe @($filter,'--test-threads=1') $name
    }
    'passed' | Set-Content C:\QAReports\targeted-tests-finished.txt
    $deadline=[DateTime]::UtcNow.AddMinutes(30)
    while(-not(Test-Path C:\QAInput\candidate.ready)){
        if([DateTime]::UtcNow -gt $deadline){throw 'Candidate package was not provided'}
        Start-Sleep -Seconds 2
    }
    & C:\QARunners\handoff-installed-guest.ps1 -BaselineLabel 'r20'
} catch {$_ | Out-String | Set-Content C:\QAReports\core-recovery-error.txt;throw}
finally {'finished' | Set-Content C:\QAReports\core-recovery-finished.txt}
