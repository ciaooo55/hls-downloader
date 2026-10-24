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
    New-Item -ItemType Directory C:\QAWork -Force | Out-Null
    Copy-Item C:\QAInput\python C:\QAWork\python -Recurse
    Copy-Item C:\QAInput\runtime C:\QAWork\runtime -Recurse
    Copy-Item C:\QAInput\candidate-engine.exe C:\QAWork\runtime\HLSDownloaderEngine.exe -Force
    Add-Content C:\QAWork\python\python312._pth 'C:\QAInput\source'
    $python='C:\QAWork\python\python.exe'
    Run-Step 'generate-1080' 'C:\QAWork\runtime\ffmpeg.exe' @('-nostdin','-y','-v','error','-f','lavfi','-i','testsrc2=size=1920x1080:rate=25','-t','20','-c:v','libx264','-preset','ultrafast','-pix_fmt','yuv420p','C:\QAWork\1080.mp4')
    $common=@('C:\QAInput\source\smoke_v7_player_rendering.py','--engine','C:\QAWork\runtime\HLSDownloaderEngine.exe','--libmpv','C:\QAWork\runtime\libmpv-2.dll','--media','C:\QAWork\1080.mp4','--environment','software','--max-drop-ratio','0.05')
    Run-Step 'correct-downscaling-no' $python ($common+@('--mpv-option','correct-downscaling=no','--report',"$report\correct-downscaling-no.json"))
    Run-Step 'fast-profile' $python ($common+@('--profile','fast','--report',"$report\fast-profile.json"))
    Run-Step 'dither-no' $python ($common+@('--mpv-option','dither=no','--report',"$report\dither-no.json"))
    Run-Step 'linear-downscaling-no' $python ($common+@('--mpv-option','linear-downscaling=no','--report',"$report\linear-downscaling-no.json"))
    Run-Step 'downscaling-both-no' $python ($common+@('--mpv-option','correct-downscaling=no','--mpv-option','linear-downscaling=no','--report',"$report\downscaling-both-no.json"))
    $summary.passed=($summary.error -eq '')
}catch{$summary.error=$_.ToString()}
finally{$summary.finished=$true;Save-Status}