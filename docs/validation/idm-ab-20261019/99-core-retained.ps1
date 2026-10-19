$work='C:\HLSValidation\idm-ab-20261019'
Get-Process | Where-Object {$_.Path -like 'C:\HLSRuntime\*'} | Stop-Process -Force
Copy-Item "$work\core-retained-tests.exe",C:\HLSBackend\vcruntime140.dll C:\HLSTests -Force
Copy-Item C:\HLSSource\assets\app-icon.ico C:\HLSTests -Force
$env:HLS_V7_DATA_DIR='C:\QAWork\retained-unit-data'
$env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\retained-unit-downloads'
$p=Start-Process C:\HLSTests\core-retained-tests.exe -ArgumentList '--test-threads=1' -NoNewWindow -PassThru -RedirectStandardOutput "$work\core-retained.log" -RedirectStandardError "$work\core-retained.stderr"
$null=$p.Handle
if(-not $p.WaitForExit(900000)){Stop-Process -Id $p.Id;throw 'Core retained timeout'}
$p.ExitCode | Set-Content "$work\core-retained.exit"
if($p.ExitCode -ne 0){throw 'Core retained tests failed'}
[ordered]@{
    os=Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,BuildNumber,TotalVisibleMemorySize
    cpu=Get-CimInstance Win32_Processor | Select-Object Name,NumberOfLogicalProcessors
    final_core_sha256=(Get-FileHash C:\HLSRuntime\app\resources\HLSDownloaderEngine.exe).Hash
    test_binary_sha256=(Get-FileHash C:\HLSTests\core-retained-tests.exe).Hash
} | ConvertTo-Json -Depth 5 | Set-Content "$work\environment-final.json"
