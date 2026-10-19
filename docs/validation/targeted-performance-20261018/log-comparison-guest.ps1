$ErrorActionPreference='Stop'
$work='C:\HLSValidation'
try {
    New-Item -ItemType Directory -Force C:\HLSTests\scripts,C:\QAWork,C:\HLSBaseline,C:\HLSCandidate | Out-Null
    Copy-Item C:\HLSSource\scripts\validation C:\HLSTests\scripts -Recurse
    Copy-Item "$work\baseline\HLSDownloaderEngine.exe" C:\HLSBaseline
    Copy-Item "$work\candidate\HLSDownloaderEngine.exe" C:\HLSCandidate
    Copy-Item C:\HLSBackend\vcruntime140.dll C:\HLSBaseline
    Copy-Item C:\HLSBackend\vcruntime140.dll C:\HLSCandidate
    $env:TEMP='C:\QAWork';$env:TMP='C:\QAWork';$env:HLS_VM_ACCEPTANCE='1'
    $env:HLS_V6_SKIP_MIGRATE='1'
    $env:PYTHONPATH='C:\HLSPrevious\python-packages;C:\HLSTests\scripts\validation'
    $env:PYTHONIOENCODING='utf-8'
    Set-Location C:\HLSTests
    $p=Start-Process C:\HLSPython\python.exe -ArgumentList @('C:\HLSTests\scripts\validation\compare_v7_progress_logs.py','--baseline','C:\HLSBaseline\HLSDownloaderEngine.exe','--candidate','C:\HLSCandidate\HLSDownloaderEngine.exe','--report',"$work\log-comparison.json") -NoNewWindow -PassThru -RedirectStandardOutput "$work\log-comparison.stdout" -RedirectStandardError "$work\log-comparison.stderr"
    $null=$p.Handle
    if(-not $p.WaitForExit(900000)){Stop-Process -Id $p.Id;throw 'comparison timed out'}
    $p.ExitCode | Set-Content "$work\log-comparison.exit"
} catch {$_|Out-String|Set-Content "$work\log-comparison.error"}