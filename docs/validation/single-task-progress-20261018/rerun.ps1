$ErrorActionPreference='Stop'
$work='C:\HLSValidation\single-task'
Start-Transcript "$work\rerun-guest.log" -Force
try {
    New-Item -ItemType Directory -Force C:\HLSTests,C:\QAWork | Out-Null
    Copy-Item "$work\core-fixed-tests.exe" C:\HLSTests\core-tests.exe
    Copy-Item C:\HLSBackend\vcruntime140.dll C:\HLSTests
    Copy-Item C:\HLSSource\assets\app-icon.ico C:\HLSTests
    $env:TEMP='C:\QAWork';$env:TMP='C:\QAWork';$env:HLS_VM_ACCEPTANCE='1'
    $env:HLS_V6_SKIP_MIGRATE='1';$env:HLS_V7_DATA_DIR='C:\QAWork\unit-data'
    $env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\unit-downloads'
    Set-Location C:\HLSTests
    $results=@()
    function Run-Test([string]$Name,[string[]]$Arguments) {
        $Name | Set-Content "$work\rerun-phase.txt"
        $p=Start-Process C:\HLSTests\core-tests.exe -ArgumentList @($Arguments | ForEach-Object {'"'+$_+'"'}) -NoNewWindow -PassThru -RedirectStandardOutput "$work\$Name.log" -RedirectStandardError "$work\$Name.stderr"
        $null=$p.Handle
        if(-not $p.WaitForExit(600000)){Stop-Process -Id $p.Id;$code=-1}else{$code=$p.ExitCode}
        $code | Set-Content "$work\$Name.exit"
        $script:results+=@{name=$Name;exit_code=$code}
        $script:results | ConvertTo-Json | Set-Content "$work\rerun-results.json"
    }
    # 单独记录 PowerShell 冷启动，避免将尚未启动的夹具误当作合并进程响应。
    $warm=[Diagnostics.Stopwatch]::StartNew()
    & powershell.exe -NoProfile -NonInteractive -Command 'Write-Output fixture-warmup' *> "$work\powershell-warmup.log"
    @{exit_code=$LASTEXITCODE;elapsed_ms=$warm.ElapsedMilliseconds} | ConvertTo-Json | Set-Content "$work\powershell-warmup.json"
    Run-Test 'core-rerun' @('--test-threads=1')
    foreach($round in 1..3) {
        Run-Test "hls-resume-$round" @('media::hls::tests::authenticated_vod_pause_resume_reuses_completed_segments','--exact','--nocapture')
        Run-Test "merge-control-$round" @('media::merge::tests::merging_pause_and_cancel_stop_the_running_child','--exact','--nocapture')
    }
    $env:HLS_FFMPEG='C:\HLSPrevious\baseline-image\HLSDownloader\app\resources\ffmpeg.exe'
    Run-Test 'real-ffmpeg-control' @('media::merge::tests::real_ffmpeg_pause_and_cancel_reap_the_child','--exact','--ignored','--nocapture')
    'finished' | Set-Content "$work\rerun.finished"
} catch {$_|Out-String|Set-Content "$work\rerun-guest.error"} finally {Stop-Transcript}
