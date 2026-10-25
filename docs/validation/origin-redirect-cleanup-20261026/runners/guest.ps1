$ErrorActionPreference = 'Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
$env:HLS_VM_ACCEPTANCE = '1'
$env:HLS_V7_CURL_IMPERSONATE = 'C:\QAInput\curl-impersonate.exe'
$env:PATH = 'C:\QAInput;' + $env:PATH
$env:TEMP = 'C:\QAWork\tmp'
$env:TMP = $env:TEMP
$env:COREPACK_ENABLE_PROJECT_SPEC = '0'
New-Item -ItemType Directory -Force -Path C:\QAWork, $env:TEMP | Out-Null
$steps = New-Object Collections.ArrayList
$summary = [ordered]@{ passed = $false; finished = $false; current = ''; steps = $steps; error = ''; curl_sha256 = (Get-FileHash C:\QAInput\curl-impersonate.exe -Algorithm SHA256).Hash }
function Save-Status { $summary | ConvertTo-Json -Depth 8 | Set-Content C:\QAReports\summary.json -Encoding UTF8 }
function Check([string]$Name, [string]$Exe, [string[]]$Arguments) {
    $summary.current = $Name; Save-Status
    $process = Start-Process -FilePath $Exe -ArgumentList $Arguments -WorkingDirectory (Get-Location).Path -WindowStyle Hidden -PassThru -RedirectStandardOutput "C:\QAReports\$Name.stdout.txt" -RedirectStandardError "C:\QAReports\$Name.stderr.txt"
    [void]$process.Handle
    if (-not $process.WaitForExit(240000)) { Stop-Process -Id $process.Id -Force; throw "$Name timed out" }
    $exitCode = $process.ExitCode
    [void]$steps.Add([ordered]@{ name = $Name; exit_code = $exitCode })
    Save-Status
    if ($exitCode -ne 0) { throw "$Name failed with $exitCode" }
}
try {
    Save-Status
    Copy-Item C:\QAInput\core-tests.exe C:\QAWork\core-tests.exe
    foreach ($name in @('vcruntime140.dll','vcruntime140_1.dll')) {
        if (Test-Path "C:\QAInput\$name") { Copy-Item "C:\QAInput\$name" C:\QAWork }
    }
    Check 'redirect-transports' C:\QAWork\core-tests.exe @('post_redirects_preserve_or_discard_the_body_across_transports','--test-threads=1','--nocapture')
    Check 'redirect-credentials' C:\QAWork\core-tests.exe @('cross_origin_redirect_does_not_forward_cookie','--test-threads=1','--nocapture')
    Check 'cleanup-player-lock' C:\QAWork\core-tests.exe @('completed_cache_cleanup_releases_player_after_recording_the_output','--test-threads=1','--nocapture')
    Check 'published-output' C:\QAWork\core-tests.exe @('a_published_','--test-threads=1','--nocapture')
    Check 'cleanup-boundaries' C:\QAWork\core-tests.exe @('cleanup','--test-threads=1')
    Check 'completed-media' C:\QAWork\core-tests.exe @('completed_media_cleans_segments','--test-threads=1','--nocapture')
    Copy-Item C:\QAInput\extension C:\QAWork\extension -Recurse
    Set-Location C:\QAWork\extension
    Check 'extension-install' C:\QAInput\node.exe @('C:\QAInput\pnpm\bin\pnpm.cjs','install','--frozen-lockfile','--store-dir','C:\QAWork\pnpm-store')
    Check 'extension-tests' C:\QAInput\node.exe @('C:\QAInput\pnpm\bin\pnpm.cjs','test')
    $summary.passed = $true
} catch { $summary.error = $_.Exception.Message }
finally { $summary.finished = $true; Save-Status }
