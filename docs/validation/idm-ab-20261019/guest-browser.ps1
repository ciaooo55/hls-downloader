$ErrorActionPreference='Stop'
$work='C:\HLSValidation\idm-ab-20261019'
Start-Transcript "$work\guest-browser.log" -Force
try {
    New-Item -ItemType Directory -Force C:\HLSTests\scripts,C:\QAWork | Out-Null
    Copy-Item C:\HLSSource\scripts\validation C:\HLSTests\scripts -Recurse
    Copy-Item C:\HLSPrevious\baseline-image\HLSDownloader C:\HLSRuntime -Recurse
    Copy-Item C:\HLSValidation\single-task\candidate\*.jar C:\HLSRuntime\app -Force
    Copy-Item "$work\candidate\*.exe" C:\HLSRuntime\app\resources -Force
    Copy-Item "$work\HLSDownloaderEngine-retained.exe" C:\HLSRuntime\app\resources\HLSDownloaderEngine.exe -Force
    Copy-Item C:\HLSSource\extension\.output C:\HLSExtension -Recurse -Force
    $env:TEMP='C:\QAWork';$env:TMP='C:\QAWork';$env:HLS_VM_ACCEPTANCE='1'
    $env:HLS_V6_SKIP_MIGRATE='1';$env:HLS_V7_DATA_DIR='C:\QAWork\browser-data'
    $env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\browser-downloads'
    $env:PYTHONPATH='C:\HLSPrevious\python-packages;C:\HLSTests\scripts\validation'
    $env:PYTHONIOENCODING='utf-8'
    $installer=Start-Process "$work\firefox-setup.exe" -ArgumentList @('/S','/InstallDirectoryPath=C:\Firefox') -PassThru -WindowStyle Hidden
    $null=$installer.Handle
    if(-not $installer.WaitForExit(120000)){throw 'Firefox installer timeout'}
    Set-Location C:\HLSTests
    $p=Start-Process C:\HLSPython\python.exe -ArgumentList ('"'+$work+'\firefox-slow-start.py"') -NoNewWindow -PassThru -RedirectStandardOutput "$work\firefox-final.log" -RedirectStandardError "$work\firefox-final.stderr"
    $null=$p.Handle
    if(-not $p.WaitForExit(180000)){Stop-Process -Id $p.Id;throw 'Firefox fixture timed out'}
    'ready' | Set-Content "$work\browser.ready"
    $browserDeadline=(Get-Date).AddHours(1)
    while((Get-Date) -lt $browserDeadline -and -not(Test-Path "$work\browser.stop")) {
        foreach($browserJob in Get-ChildItem "$work\browser-queue" -Filter '*.ps1' -ErrorAction SilentlyContinue | Sort-Object Name) {
            if(Test-Path ($browserJob.FullName+'.exit')){continue}
            try {& $browserJob.FullName *> ($browserJob.FullName+'.log'); '0' | Set-Content ($browserJob.FullName+'.exit')}
            catch {$_ | Out-String | Add-Content ($browserJob.FullName+'.log'); '1' | Set-Content ($browserJob.FullName+'.exit')}
        }
        Start-Sleep -Milliseconds 500
    }
} catch {$_|Out-String|Set-Content "$work\guest-browser.error"} finally {Stop-Transcript}
