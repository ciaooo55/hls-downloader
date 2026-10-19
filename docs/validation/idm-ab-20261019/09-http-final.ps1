$work='C:\HLSValidation\idm-ab-20261019'
Get-Process | Where-Object {$_.Path -like 'C:\HLSRuntime\*'} | Stop-Process -Force
Copy-Item "$work\HLSDownloaderEngine-before-startup.exe" C:\HLSRuntime\app\resources\HLSDownloaderEngine.exe -Force
function Invoke-FinalHttp([string]$Name,[string[]]$TestArguments) {
    $process=Start-Process C:\HLSPython\python.exe -ArgumentList @($TestArguments | ForEach-Object {'"'+$_+'"'}) -NoNewWindow -PassThru -RedirectStandardOutput "$work\$Name.log" -RedirectStandardError "$work\$Name.stderr"
    $null=$process.Handle
    if(-not $process.WaitForExit(900000)){Stop-Process -Id $process.Id;throw "$Name timed out"}
    $process.ExitCode | Set-Content "$work\$Name.exit"
    if($process.ExitCode -ne 0){throw "$Name failed: $($process.ExitCode)"}
}
$scripts='C:\HLSTests\scripts\validation'
$engine='C:\HLSRuntime\app\resources\HLSDownloaderEngine.exe'
Invoke-FinalHttp 'http-boundaries-final' @("$scripts\smoke_v7_http_boundaries.py",'--engine',$engine,'--report',"$work\http-boundaries-final.json")
Invoke-FinalHttp 'http-stall-final' @("$scripts\smoke_v7_http_stall.py",'--engine',$engine,'--root','C:\QAWork\http-stall-final','--report',"$work\http-stall-final.json")
