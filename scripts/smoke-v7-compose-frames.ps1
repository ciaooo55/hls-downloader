[CmdletBinding()]
param(
    [string]$ReportPath = '',
    [string]$AppPath = '',
    [int]$Width = 1024,
    [int]$Height = 600,
    [int]$TimeoutSeconds = 180
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
$desktop = Join-Path $repo 'desktop_ui'
$reportDir = Join-Path $repo 'artifacts\v7-productization\performance'
if ([string]::IsNullOrWhiteSpace($ReportPath)) {
    $ReportPath = Join-Path $reportDir 'compose-1000-task-frames.json'
}
$ReportPath = [IO.Path]::GetFullPath($ReportPath)
$stdoutPath = Join-Path $reportDir 'compose-frame-audit.stdout.log'
$stderrPath = Join-Path $reportDir 'compose-frame-audit.stderr.log'
New-Item -ItemType Directory -Force -Path $reportDir | Out-Null
[IO.File]::Delete($ReportPath)
[IO.File]::Delete($stdoutPath)
[IO.File]::Delete($stderrPath)

# Verification uses the same cache contract as build/bootstrap so it is portable
# across developer machines and clean CI runners.
if ($env:HLS_V7_BUILD_CACHE -and -not [IO.Path]::IsPathRooted($env:HLS_V7_BUILD_CACHE)) {
    throw 'HLS_V7_BUILD_CACHE must be an absolute path.'
}
$cacheRoot = if ($env:HLS_V7_BUILD_CACHE) {
    [IO.Path]::GetFullPath($env:HLS_V7_BUILD_CACHE)
} else {
    Join-Path $repo '.tool-cache\build-cache'
}
$jdkRoot = $env:HLS_V7_JAVA_HOME
if(-not $jdkRoot -and (Test-Path (Join-Path $cacheRoot 'jdk-21\bin\java.exe'))){ $jdkRoot = Join-Path $cacheRoot 'jdk-21' }
if(-not $jdkRoot){ throw 'JDK 21 was not found. Set HLS_V7_JAVA_HOME or run scripts\bootstrap-v7-toolchain.ps1.' }
$env:JAVA_HOME = $jdkRoot
$env:GRADLE_USER_HOME = Join-Path $cacheRoot 'gradle'
$env:HLS_UI_AUDIT_SURFACE = 'tasks_1000'
$env:HLS_UI_AUDIT_WIDTH = [string]$Width
$env:HLS_UI_AUDIT_HEIGHT = [string]$Height
$env:HLS_UI_FRAME_REPORT = $ReportPath
$env:HLS_V6_SKIP_MIGRATE = '1'

$existingAppIds = @(Get-CimInstance Win32_Process | Where-Object {
    $_.Name -eq 'java.exe' -and $_.CommandLine -like '*com.hlsdownloader.desktop.MainKt*'
} | ForEach-Object { $_.ProcessId })
$productProcessNames = @('HLSDownloader.exe', 'HLSDownloaderEngine.exe', 'HLSDownloaderPresenter.exe', 'HLSDownloaderNativeHost.exe')
$existingProductIds = @(Get-CimInstance Win32_Process | Where-Object {
    $_.Name -in $productProcessNames
} | ForEach-Object { $_.ProcessId })
$runner = $null
try {
    if ([string]::IsNullOrWhiteSpace($AppPath)) {
        $runner = Start-Process -FilePath $env:ComSpec -ArgumentList @(
            '/d', '/c', 'gradlew.bat run --console=plain --no-daemon'
        ) -WorkingDirectory $desktop -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
    } else {
        $AppPath = [IO.Path]::GetFullPath($AppPath)
        if (-not (Test-Path -LiteralPath $AppPath -PathType Leaf)) { throw "Compose candidate is missing: $AppPath" }
        $runner = Start-Process -FilePath $AppPath -WorkingDirectory ([IO.Path]::GetDirectoryName($AppPath)) -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
    }
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while (-not (Test-Path -LiteralPath $ReportPath) -and [DateTime]::UtcNow -lt $deadline) {
        if ($runner.HasExited) {
            $stdout = if (Test-Path -LiteralPath $stdoutPath) { [IO.File]::ReadAllText($stdoutPath, [Text.Encoding]::UTF8) } else { '' }
            $stderr = if (Test-Path -LiteralPath $stderrPath) { [IO.File]::ReadAllText($stderrPath, [Text.Encoding]::UTF8) } else { '' }
            throw "Compose audit exited before producing a report (exit $($runner.ExitCode)).`n$stdout`n$stderr"
        }
        Start-Sleep -Milliseconds 250
    }
    if (-not (Test-Path -LiteralPath $ReportPath)) {
        throw "Compose audit did not produce $ReportPath within $TimeoutSeconds seconds."
    }
    $result = [IO.File]::ReadAllText($ReportPath, [Text.Encoding]::UTF8) | ConvertFrom-Json
    if ($result.task_count -ne 1000) { throw "Compose audit used $($result.task_count) tasks, expected 1000." }
    if ($result.window_width -ne $Width -or $result.window_height -ne $Height) {
        throw "Compose audit reported $($result.window_width)x$($result.window_height), expected ${Width}x${Height}."
    }
    if (-not $result.passed -or [double]$result.frame_p95_ms -gt [double]$result.threshold_ms) {
        throw "Compose frame threshold failed: $($result | ConvertTo-Json -Compress)"
    }
    Write-Host ($result | ConvertTo-Json -Compress)
} finally {
    if ($runner -and -not $runner.HasExited) {
        & taskkill.exe /PID $runner.Id /T /F 2>$null | Out-Null
        Wait-Process -Id $runner.Id -Timeout 5 -ErrorAction SilentlyContinue
    }
    $newApps = @(Get-CimInstance Win32_Process | Where-Object {
        $_.Name -eq 'java.exe' -and
        $_.CommandLine -like '*com.hlsdownloader.desktop.MainKt*' -and
        $_.ProcessId -notin $existingAppIds
    })
    foreach ($app in $newApps) {
        Stop-Process -Id $app.ProcessId -Force -ErrorAction SilentlyContinue
    }
    $newProductProcesses = @(Get-CimInstance Win32_Process | Where-Object {
        $_.Name -in $productProcessNames -and $_.ProcessId -notin $existingProductIds
    } | Sort-Object { if ($_.Name -eq 'HLSDownloader.exe') { 0 } else { 1 } })
    foreach ($process in $newProductProcesses) {
        Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
        Wait-Process -Id $process.ProcessId -Timeout 5 -ErrorAction SilentlyContinue
    }
}
