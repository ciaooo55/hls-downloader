$ErrorActionPreference = 'Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
$env:HLS_VM_ACCEPTANCE = '1'
$env:TEMP = 'C:\QAAdmissionWork\temp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force $env:TEMP | Out-Null
$reportRoot = 'C:\QAAdmissionReports'
$result = [ordered]@{ passed = $false; finished = $false; scope = 'Core/Native Host targeted regression; no physical UI acceptance'; checks = @() }
function Save-Result {
    $result | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath "$reportRoot\summary.json" -Encoding UTF8
}
try {
    $source = 'C:\QAAdmissionInput\CoreTests.exe'
    $expected = (Get-Content -LiteralPath 'C:\QAAdmissionInput\test-binary.sha256' -Raw).Trim()
    $result.test_binary_sha256 = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($result.test_binary_sha256 -ne $expected) { throw 'Test executable hash mismatch' }
    Copy-Item -LiteralPath $source -Destination 'C:\QAAdmissionWork\CoreTests.exe'
    $runtime = 'C:\QAAdmissionInput\vcruntime140.dll'
    $result.runtime_sha256 = (Get-FileHash -LiteralPath $runtime -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($result.runtime_sha256 -ne (Get-Content -LiteralPath 'C:\QAAdmissionInput\runtime.sha256' -Raw).Trim()) { throw 'CRT hash mismatch' }
    Copy-Item -LiteralPath $runtime -Destination 'C:\QAAdmissionWork\vcruntime140.dll'
    foreach ($check in @(
        @{ name = 'atomic-admission'; filter = 'handoff_admission' },
        @{ name = 'native-host'; filter = 'native_host::tests::' }
    )) {
        $result.phase = $check.name
        Save-Result
        $process = Start-Process -FilePath 'C:\QAAdmissionWork\CoreTests.exe' -ArgumentList @($check.filter, '--test-threads=1') -WindowStyle Hidden -PassThru -RedirectStandardOutput "$reportRoot\$($check.name).stdout.txt" -RedirectStandardError "$reportRoot\$($check.name).stderr.txt"
        [void]$process.Handle
        if (-not $process.WaitForExit(90000)) {
            Stop-Process -Id $process.Id -Force
            throw "$($check.name) timed out"
        }
        $result.checks += @{ name = $check.name; exit_code = $process.ExitCode }
        if ($process.ExitCode -ne 0) { throw "$($check.name) failed: $($process.ExitCode)" }
    }
    $result.passed = $true
} catch {
    $result.error = $_.Exception.Message
} finally {
    $result.finished = $true
    Save-Result
}
