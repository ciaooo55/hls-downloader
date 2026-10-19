$ErrorActionPreference='Stop'
$work='C:\HLSLayout'
try {
    Copy-Item 'C:\HLSBaseline\HLSDownloader' 'C:\HLSRuntime' -Recurse
    Copy-Item "$work\HLSDownloaderDesktop-7.0.3.jar" 'C:\HLSRuntime\app' -Force
    $env:HLS_UI_AUDIT_SURFACE='tasks_1000'
    $env:HLS_UI_AUDIT_THEME='light'
    $env:HLS_UI_AUDIT_WIDTH='1200'
    $env:HLS_UI_AUDIT_HEIGHT='600'
    $env:HLS_UI_TEST_API='1'
    $env:HLS_UI_TEST_PORT='19861'
    $env:HLS_UI_TEST_TOKEN='ui-layout-20261019-local'
    $env:HLS_V7_DATA_DIR='C:\HLSLayoutData'
    $env:HLS_V7_DOWNLOAD_DIR='C:\HLSLayoutDownloads'
    $env:HLS_V6_SKIP_MIGRATE='1'
    $env:TEMP='C:\HLSLayoutTemp'
    $env:TMP=$env:TEMP
    New-Item -ItemType Directory -Force $env:TEMP | Out-Null
    Start-Process 'C:\HLSRuntime\HLSDownloader.exe' -WindowStyle Hidden
    'ready' | Set-Content "$work\ready"
    $deadline=(Get-Date).AddMinutes(40)
    while((Get-Date) -lt $deadline -and -not(Test-Path "$work\stop")) {
        foreach($job in Get-ChildItem "$work\jobs" -Filter '*.ps1' | Sort-Object Name) {
            if(Test-Path ($job.FullName+'.exit')){continue}
            try { & $job.FullName *> ($job.FullName+'.log'); '0' | Set-Content ($job.FullName+'.exit') }
            catch { $_ | Out-String | Set-Content ($job.FullName+'.log'); '1' | Set-Content ($job.FullName+'.exit') }
        }
        Start-Sleep -Milliseconds 300
    }
} catch { $_ | Out-String | Set-Content "$work\guest.error" }
