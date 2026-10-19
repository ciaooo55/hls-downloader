param(
    [int]$TimeoutSeconds = 12,
    [string]$InstallDir = "",
    [switch]$IncludeNativeHost,
    [switch]$RequireCheckpoint
)

$ErrorActionPreference = "SilentlyContinue"
$script:processPathCache = @{}

function Get-ProcessExecutablePath {
    param([System.Diagnostics.Process]$Process)

    $cacheKey = [string]$Process.Id
    if ($script:processPathCache.ContainsKey($cacheKey)) {
        return [string]$script:processPathCache[$cacheKey]
    }
    $resolved = ""
    try {
        $resolved = [string]$Process.Path
    } catch {
    }
    # A 32-bit caller cannot always read MainModule/Path for a 64-bit target.
    # CIM remains architecture-neutral and keeps shutdown scoped to InstallDir.
    # Bound and cache this fallback: repeatedly starting an unbounded WMI query
    # from the installer progress page was able to exceed the advertised
    # shutdown timeout and made an otherwise healthy cover upgrade look hung.
    if (-not $resolved) {
        try {
            $resolved = [string](Get-CimInstance Win32_Process `
                -Filter "ProcessId = $($Process.Id)" `
                -OperationTimeoutSec 2 `
                -ErrorAction Stop).ExecutablePath
        } catch {
            $resolved = ""
        }
    }
    $script:processPathCache[$cacheKey] = $resolved
    return $resolved
}

function Get-TargetProcesses {
    param([string[]]$Names)

    $processes = @(Get-Process -Name $Names -ErrorAction SilentlyContinue)
    if (-not $InstallDir) { return $processes }
    try {
        $installRoot = [IO.Path]::GetFullPath($InstallDir)
        $directorySeparator = [string][IO.Path]::DirectorySeparatorChar
        if (-not $installRoot.EndsWith($directorySeparator, [System.StringComparison]::Ordinal)) {
            $installRoot += $directorySeparator
        }
    } catch {
        return @()
    }
    return @(
        $processes | Where-Object {
            try {
                $processPath = Get-ProcessExecutablePath $_
                $processPath -and [IO.Path]::GetFullPath($processPath).StartsWith(
                    $installRoot,
                    [System.StringComparison]::OrdinalIgnoreCase
                )
            } catch {
                $false
            }
        }
    )
}

$targetProcessNames = @("HLSDownloader", "HLSDownloaderEngine", "hls-downloader-engine", "HLSDownloaderPresenter", "HLSDownloaderCore", "HLSNativeShell", "hls-native-shell", "HLSNativeEngine", "hls-native-engine")
if ($IncludeNativeHost) { $targetProcessNames += "HLSDownloaderNativeHost*" }
$targetRunningAtStart = @(Get-TargetProcesses $targetProcessNames)
$overallDeadline = [DateTime]::UtcNow.AddSeconds([Math]::Max(3, $TimeoutSeconds))
$shutdownEngine = ""
$runningEngine = $targetRunningAtStart |
    Where-Object { $_.ProcessName -in @("HLSDownloaderEngine", "hls-downloader-engine") } |
    Select-Object -First 1
$gracefulCoreExit = "not-running"
# 先关闭会重新连接 Core 的工作台及桥接进程，防止停机期间再次启动引擎。
if ($RequireCheckpoint) {
    if (-not $InstallDir) { Write-Error '升级停机必须指定安装目录'; exit 1 }
    foreach ($desktop in @(Get-TargetProcesses @('HLSDownloader'))) {
        $desktop.CloseMainWindow() | Out-Null
    }
    Start-Sleep -Milliseconds 300
    foreach ($desktop in @(Get-TargetProcesses @('HLSDownloader'))) {
        # 不使用 /T：Core 可能是其子进程，必须先单独保存续传状态。
        $desktop | Stop-Process -Force
    }
    Get-TargetProcesses @('HLSDownloaderNativeHost*') | Stop-Process -Force
}
if ($runningEngine) {
    $shutdownEngine = Get-ProcessExecutablePath $runningEngine
    if (-not $shutdownEngine -and $InstallDir) {
        $shutdownEngine = Join-Path $InstallDir "app\resources\HLSDownloaderEngine.exe"
    }
}
if ($shutdownEngine -and (Test-Path -LiteralPath $shutdownEngine -PathType Leaf)) {
    # v7 has no HTTP supervisor. Ask the resident Core to checkpoint and stop
    # through its versioned named-pipe contract before the bounded kill fallback.
    # Engine 是窗口型 EXE；显式等待，避免把尚未返回的空 LASTEXITCODE 当成失败。
    $shutdownRequest = Start-Process -FilePath $shutdownEngine -ArgumentList '--shutdown' -WindowStyle Hidden -PassThru
    if ($shutdownRequest.WaitForExit(8000)) {
        $gracefulCoreExit = [string]$shutdownRequest.ExitCode
    } else {
        $shutdownRequest | Stop-Process -Force
        $gracefulCoreExit = 'timeout'
    }
    if ($RequireCheckpoint -and $gracefulCoreExit -ne '0' -and
        @(Get-TargetProcesses @('HLSDownloaderEngine', 'hls-downloader-engine')).Count -gt 0) {
        Write-Output '任务状态未能安全保存，已停止升级；旧版本仍保留。'
        exit 1
    }
}

$gracefulDeadline = [DateTime]::UtcNow.AddSeconds([Math]::Min(4, [Math]::Max(1, $TimeoutSeconds)))
if ($gracefulDeadline -gt $overallDeadline) { $gracefulDeadline = $overallDeadline }
while ([DateTime]::UtcNow -lt $gracefulDeadline) {
    if (-not (Get-TargetProcesses @("HLSDownloader"))) { break }
    Start-Sleep -Milliseconds 200
}

foreach ($desktop in @(Get-TargetProcesses @("HLSDownloader"))) {
    & "$env:SystemRoot\System32\taskkill.exe" /PID $desktop.Id /T /F 2>$null | Out-Null
    $desktop | Stop-Process -Force
}
Get-TargetProcesses @("HLSDownloaderCore") | Stop-Process -Force

if ($IncludeNativeHost) {
    # Versioned hosts are named HLSDownloaderNativeHost-<version>.exe.  This
    # 卸载和升级预停机都必须关闭桥接进程，防止它在事务期间再次启动 Core。
    Get-TargetProcesses @("HLSDownloaderNativeHost*") | Stop-Process -Force
}

function Test-ApplicationFilesWritable {
    $script:lastLockedFile = ""
    $script:lastLockError = ""
    if (-not $InstallDir) { return $true }
    # The Native Messaging host is deliberately excluded.  A browser can keep
    # it alive indefinitely, and the installer now deploys it under a new
    # versioned name rather than replacing that locked executable.
    foreach ($name in @("HLSDownloader.exe", "HLSDownloaderCore.exe", "HLSNativeShell.exe", "HLSNativeEngine.exe")) {
        $target = Join-Path $InstallDir $name
        if (-not (Test-Path -LiteralPath $target)) { continue }
        try {
            $stream = [System.IO.File]::Open(
                $target,
                [System.IO.FileMode]::Open,
                [System.IO.FileAccess]::Read,
                [System.IO.FileShare]::None
            )
            $stream.Dispose()
        } catch {
            $script:lastLockedFile = $target
            $script:lastLockError = $_.Exception.Message
            return $false
        }
    }
    return $true
}

do {
    $running = Get-TargetProcesses $targetProcessNames
    if (-not $running -and (Test-ApplicationFilesWritable)) { exit 0 }
    if ($running) {
        $running | Stop-Process -Force
    }
    Start-Sleep -Milliseconds 250
} while ([DateTime]::UtcNow -lt $overallDeadline)
$remaining = @(Get-TargetProcesses $targetProcessNames)
$remainingSummary = ($remaining | ForEach-Object {
    $path = Get-ProcessExecutablePath $_
    "$($_.ProcessName):$($_.Id):$path"
}) -join "; "
$writable = Test-ApplicationFilesWritable
$installDirBase64 = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes([string]$InstallDir))
$namedDiagnostics = @(Get-Process -Name $targetProcessNames -ErrorAction SilentlyContinue | ForEach-Object {
    $path = Get-ProcessExecutablePath $_
    $pathBase64 = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($path))
    "$($_.ProcessName):$($_.Id):$pathBase64"
}) -join "; "
$lockedFileBase64 = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes([string]$lastLockedFile))
Write-Output "Shutdown timeout; install_dir=$InstallDir; install_dir_utf16=$installDirBase64; graceful_core_exit=$gracefulCoreExit; remaining=$remainingSummary; named=$namedDiagnostics; files_writable=$writable; locked_file_utf16=$lockedFileBase64; lock_error=$lastLockError"
exit 1
