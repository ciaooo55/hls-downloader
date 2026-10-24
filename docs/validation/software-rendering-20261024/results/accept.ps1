param([ValidateSet('software','gpu')][string]$Mode)
$ErrorActionPreference='Stop'
if($env:USERNAME -ne 'WDAGUtilityAccount'){throw 'Sandbox only'}
$env:HLS_VM_ACCEPTANCE='1'; $env:HLS_V7_DATA_DIR='C:\QAWork\data'
$env:PYTHONDONTWRITEBYTECODE='1'; $report='C:\QAReports'
$steps=New-Object Collections.ArrayList
$summary=[ordered]@{mode=$Mode;passed=$false;finished=$false;steps=$steps;error=''}
function Save-Status {$summary | ConvertTo-Json -Depth 8 | Set-Content "$report\summary.json" -Encoding UTF8}
function Run-Step([string]$Name,[string]$Exe,[string[]]$Arguments,[int]$Timeout=100){
    $summary.current=$Name; Save-Status
    $quoted=@($Arguments|ForEach-Object {if($_ -match '\s'){'"'+$_+'"'}else{$_}})
    $process=Start-Process $Exe -ArgumentList $quoted -WindowStyle Hidden -PassThru -RedirectStandardOutput "$report\$Name.stdout.log" -RedirectStandardError "$report\$Name.stderr.log"
    [void]$process.Handle
    if(-not $process.WaitForExit($Timeout*1000)){& taskkill.exe /PID $process.Id /T /F|Out-Null;throw "$Name timeout"}
    [void]$steps.Add([ordered]@{name=$Name;exit_code=$process.ExitCode})
    Save-Status
    if($process.ExitCode -ne 0){$summary.error+="$Name failed; "}
}
try{
    Save-Status
    $deadline=(Get-Date).AddMinutes(12)
    while(-not(Test-Path C:\QAInput\final-ready)){if((Get-Date) -gt $deadline){throw 'Build not ready'};Start-Sleep -Seconds 2}
    New-Item -ItemType Directory -Path C:\QAWork -Force|Out-Null
    Copy-Item C:\QAInput\python C:\QAWork\python -Recurse
    Copy-Item C:\QAInput\runtime C:\QAWork\baseline -Recurse
    Copy-Item C:\QAInput\full-runtime C:\QAWork\runtime -Recurse
    Copy-Item C:\QAInput\candidate-engine.exe C:\QAWork\runtime\app\resources\HLSDownloaderEngine.exe -Force
    Add-Content C:\QAWork\python\python312._pth 'C:\QAInput\source'
    $python='C:\QAWork\python\python.exe'; $engine='C:\QAWork\runtime\app\resources\HLSDownloaderEngine.exe'
    $lib='C:\QAWork\runtime\app\resources\libmpv-2.dll'; $ffmpeg='C:\QAWork\runtime\app\resources\ffmpeg.exe'
    foreach($size in @('640x360','1920x1080')){
        Run-Step "generate-$size" $ffmpeg @('-nostdin','-y','-v','error','-f','lavfi','-i',"testsrc2=size=${size}:rate=25",'-t','20','-c:v','libx264','-preset','ultrafast','-pix_fmt','yuv420p',"C:\QAWork\$size.mp4")
        $common=@('C:\QAInput\source\smoke_v7_player_rendering.py','--libmpv',$lib,'--media',"C:\QAWork\$size.mp4",'--environment',$Mode,'--expected-hwdec','auto-safe')
        Run-Step "baseline-$size" $python ($common+@('--engine','C:\QAWork\baseline\HLSDownloaderEngine.exe','--expected-scale','lanczos','--report',"$report\baseline-$size.json"))
        $scale=if($Mode -eq 'software'){'bilinear'}else{'lanczos'}
        Run-Step "candidate-$size" $python ($common+@('--engine',$engine,'--expected-scale',$scale,'--max-drop-ratio','0.05','--report',"$report\candidate-$size.json"))
    }
    Run-Step 'download-playback-cleanup' $python @('C:\QAInput\source\smoke_v7_download_playback.py','--runtime','C:\QAWork\runtime','--report',"$report\download-playback-cleanup.json") 360
    if($Mode -eq 'gpu'){
        $summary.current='waiting-for-package'; Save-Status
        $deadline=(Get-Date).AddMinutes(15)
        while(-not(Test-Path C:\QAInput\package-ready)){if((Get-Date) -gt $deadline){throw 'Package not ready'};Start-Sleep -Seconds 2}
        Run-Step 'installer' $python @('C:\QAInput\source\smoke_v7_installed_footprint.py','--msi','C:\QAInput\candidate.msi','--manifest','C:\QAInput\candidate-manifest.json','--expected-components','C:\QAInput\expected-components.json','--report',"$report\installer.json") 360
    }
    $summary.passed=($summary.error -eq '')
}catch{$summary.error=$_.ToString()}
finally{$summary.finished=$true;Save-Status}
