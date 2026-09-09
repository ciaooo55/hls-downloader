param(
    [int]$TimeoutSeconds = 12,
    [string]$InstallDir = "",
    [switch]$IncludeNativeHost
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
if ($runningEngine) {
    $shutdownEngine = Get-ProcessExecutablePath $runningEngine
    if (-not $shutdownEngine -and $InstallDir) {
        $shutdownEngine = Join-Path $InstallDir "app\resources\HLSDownloaderEngine.exe"
    }
}
if ($shutdownEngine -and (Test-Path -LiteralPath $shutdownEngine -PathType Leaf)) {
    # v7 has no HTTP supervisor. Ask the resident Core to checkpoint and stop
    # through its versioned named-pipe contract before the bounded kill fallback.
    & $shutdownEngine --shutdown
    $gracefulCoreExit = [string]$LASTEXITCODE
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
    # path is used only by uninstallation after browser registration is gone;
    # updates deliberately leave browser-owned hosts alone.
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
                [System.IO.FileAccess]::ReadWrite,
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
