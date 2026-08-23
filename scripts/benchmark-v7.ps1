[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
$reportDir = Join-Path $repo 'artifacts\v7-productization\performance'
$reportPath = Join-Path $reportDir 'v7-performance-latest.json'
New-Item -ItemType Directory -Force -Path $reportDir | Out-Null
$env:CARGO_HOME = 'E:\HLSDownloaderBuildCache\cargo'
$env:CARGO_TARGET_DIR = 'D:\HLSDownloaderBuildCache\cargo-target'
$env:GRADLE_USER_HOME = 'E:\HLSDownloaderBuildCache\gradle'
$env:JAVA_HOME = 'E:\HLSDownloaderBuildCache\jdk-21'

function Invoke-Captured {
    param([string]$File, [string[]]$Arguments, [string]$WorkingDirectory)
    Push-Location $WorkingDirectory
    try {
        # Native Cargo/Gradle warnings are stderr diagnostics, not command
        # failures. Capture both streams without promoting warning records to
        # terminating PowerShell errors under $ErrorActionPreference=Stop.
        $previousErrorAction = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $output = (& $File @Arguments 2>&1 | ForEach-Object { $_.ToString() }) -join [Environment]::NewLine
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previousErrorAction
    } finally { Pop-Location }
    if ($exitCode -ne 0) { throw "$File failed with exit $exitCode`n$output" }
    return $output
}

$cargoCandidates = @(
    (Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'),
    (Join-Path $env:USERPROFILE '.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\cargo.exe')
)
$cargoExe = $cargoCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $cargoExe) {
    $cargoCommand = Get-Command cargo.exe -ErrorAction SilentlyContinue
    if ($cargoCommand) { $cargoExe = $cargoCommand.Source }
}
if (-not $cargoExe) { throw 'Rust cargo.exe was not found in rustup or PATH.' }
$cargoOutput = Invoke-Captured $cargoExe @(
    'test','--manifest-path','native_shell\Cargo.toml','core_server::tests::warm_core_ipc_command_p95_stays_under_75ms', '--', '--nocapture'
) $repo
$ipcMatch = [regex]::Match($cargoOutput, 'warm_core_ipc_p95_ms=([0-9.]+)')
if (-not $ipcMatch.Success) { throw 'Core IPC benchmark did not emit a P95 measurement.' }
$ipcP95 = [double]$ipcMatch.Groups[1].Value

$gradleOutput = Invoke-Captured (Join-Path $repo 'desktop_ui\gradlew.bat') @(
    'test','--tests','com.hlsdownloader.desktop.PerformanceModelTest','--rerun-tasks','--info','--console=plain'
) (Join-Path $repo 'desktop_ui')
$uiMatch = [regex]::Match($gradleOutput, 'thousand_task_model_p95_ms=([0-9.]+)')
if (-not $uiMatch.Success) {
    [IO.File]::WriteAllText((Join-Path $reportDir 'compose-benchmark.log'), $gradleOutput, [Text.UTF8Encoding]::new($false))
    throw 'Compose 1000-task benchmark did not emit a P95 measurement.'
}
$uiP95 = [double]$uiMatch.Groups[1].Value
$frameReport = Join-Path $reportDir 'compose-1000-task-frames.json'
Invoke-Captured 'powershell.exe' @(
    '-NoProfile','-ExecutionPolicy','Bypass','-File','scripts\smoke-v7-compose-frames.ps1','-ReportPath',$frameReport
) $repo | Out-Null
$frameData = Get-Content -LiteralPath $frameReport -Raw -Encoding UTF8 | ConvertFrom-Json

$hostReport = Join-Path $reportDir 'native-host-cold-start.json'
$nativeHostExe = Join-Path $env:CARGO_TARGET_DIR 'debug\HLSDownloaderNativeHost.exe'
$engine = Join-Path $env:CARGO_TARGET_DIR 'debug\hls-downloader-engine.exe'
if (-not (Test-Path $nativeHostExe) -or -not (Test-Path $engine)) {
    throw 'Build v7 debug engine and Native Host before running the benchmark.'
}
Invoke-Captured 'C:\Users\lee\.conda\envs\test\python.exe' @('scripts\smoke_v7_native_host.py','--host',$nativeHostExe,'--engine',$engine,'--report',$hostReport) $repo | Out-Null
$hostData = Get-Content -LiteralPath $hostReport -Raw -Encoding UTF8 | ConvertFrom-Json
$transferReport = Join-Path $reportDir 'real-transfer-latest.json'
Invoke-Captured $cargoExe @(
    'build','--release','--manifest-path','native_shell\Cargo.toml','--bin','hls-downloader-engine'
) $repo | Out-Null
$releaseEngine = Join-Path $env:CARGO_TARGET_DIR 'release\hls-downloader-engine.exe'
Invoke-Captured 'C:\Users\lee\.conda\envs\test\python.exe' @(
    'scripts\smoke_v7_transfer_performance.py','--engine',$releaseEngine,'--report',$transferReport
) $repo | Out-Null
$transferData = Get-Content -LiteralPath $transferReport -Raw -Encoding UTF8 | ConvertFrom-Json
$result = [ordered]@{
    schema = 1
    product_version = '7.0.0'
    measured_at = [DateTime]::UtcNow.ToString('o')
    thousand_task_model_p95_ms = [math]::Round($uiP95, 3)
    thousand_task_frame_p95_ms = $frameData.frame_p95_ms
    ipc_command_p95_ms = [math]::Round($ipcP95, 3)
    native_host_cold_start_ms = $hostData.cold_first_response_ms
    real_transfer_throughput_mib_s = $transferData.throughput_mib_s
    real_transfer_working_set_growth_mib = $transferData.working_set_growth_mib
    post_publish_extra_network_bytes = $transferData.post_publish_extra_network_bytes
    thresholds = [ordered]@{
        thousand_task_model_p95_ms = 100
        thousand_task_frame_p95_ms = 33
        ipc_command_p95_ms = 75
        native_host_cold_start_ms = 1500
        minimum_local_throughput_mib_s = 20
        maximum_working_set_growth_mib = 256
        post_publish_extra_network_bytes = 0
    }
    passed = ($uiP95 -le 100 -and $frameData.passed -and $ipcP95 -le 75 -and $hostData.cold_first_response_ms -le 1500 -and $transferData.passed)
}
[IO.File]::WriteAllText($reportPath, ($result | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
if (-not $result.passed) { throw "v7 performance threshold failed: $($result | ConvertTo-Json -Compress)" }
Write-Host ($result | ConvertTo-Json -Compress)
