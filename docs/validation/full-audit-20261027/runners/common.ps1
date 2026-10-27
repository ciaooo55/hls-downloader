$ErrorActionPreference='Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
Set-Location C:\QAWork
$env:HLS_VM_ACCEPTANCE='1'
$env:PYTHONDONTWRITEBYTECODE='1'
$env:HLS_FFMPEG='C:\QAPerf\runtime\app\resources\ffmpeg.exe'
$env:HLS_V7_CURL_IMPERSONATE='C:\QAPerf\runtime\app\resources\tools\curl-impersonate\curl-impersonate.exe'
$env:HLS_V7_LIBMPV='C:\QAPerf\runtime\app\resources\libmpv-2.dll'
function Run([string]$exe,[string[]]$arguments,[string]$name) {
    $quoted = @($arguments | ForEach-Object { '"' + ($_ -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"' })
    $p=Start-Process $exe -ArgumentList $quoted -WindowStyle Hidden -PassThru -RedirectStandardOutput "C:\QAReports\$name.stdout.txt" -RedirectStandardError "C:\QAReports\$name.stderr.txt"
    [void]$p.Handle
    if (-not $p.WaitForExit(600000)) { Stop-Process -Id $p.Id -Force; throw "$name timeout" }
    if ($p.ExitCode -ne 0) { throw "$name failed: $($p.ExitCode)" }
}
