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

$targetProcessNames = @("HLSDownloader", "HLSDownloaderCore")
if ($IncludeNativeHost) { $targetProcessNames += "HLSDownloaderNativeHost*" }
$targetRunningAtStart = @(Get-TargetProcesses $targetProcessNames)
$overallDeadline = [DateTime]::UtcNow.AddSeconds([Math]::Max(3, $TimeoutSeconds))
$configPaths = @()
if ($InstallDir) { $configPaths += Join-Path $InstallDir "config.json" }
# An installed build stores its IPC credential under LocalAppData. Only read it
# when the requested install is actually the one that is running; otherwise a
# portable/temporary upgrade could accidentally shut down another installation.
if ((-not $InstallDir -or $targetRunningAtStart.Count) -and $env:LOCALAPPDATA) {
    $configPaths += Join-Path $env:LOCALAPPDATA "HLS Downloader\config.json"
}
$token = ""
$port = 8765
foreach ($configPath in $configPaths) {
    if (-not (Test-Path -LiteralPath $configPath)) { continue }
    try {
        $configured = Get-Content -LiteralPath $configPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($configured.token) { $token = [string]$configured.token }
        if ($configured.port) { $port = [int]$configured.port }
        break
    } catch {
    }
}

if ($token) {
    try {
        Invoke-RestMethod `
            -Method Post `
            -Uri "http://127.0.0.1:$port/api/app/shutdown?resume_tasks=true" `
            -Headers @{ "X-Token" = $token } `
            -ContentType "application/json" `
            -Body "{}" `
            -TimeoutSec 3 | Out-Null
    } catch {
    }
}

$gracefulDeadline = [DateTime]::UtcNow.AddSeconds([Math]::Min(4, [Math]::Max(1, $TimeoutSeconds)))
if ($gracefulDeadline -gt $overallDeadline) { $gracefulDeadline = $overallDeadline }
while ([DateTime]::UtcNow -lt $gracefulDeadline) {
    if (-not (Get-TargetProcesses @("HLSDownloader"))) { break }
    Start-Sleep -Milliseconds 200
}

foreach ($desktop in @(Get-TargetProcesses @("HLSDownloader"))) {
    & "$env:SystemRoot\System32\taskkill.exe" /PID $desktop.Id /T /F | Out-Null
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
    foreach ($name in @("HLSDownloader.exe", "HLSDownloaderCore.exe")) {
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
Write-Output "Shutdown timeout; install_dir=$InstallDir; install_dir_utf16=$installDirBase64; remaining=$remainingSummary; named=$namedDiagnostics; files_writable=$writable; locked_file_utf16=$lockedFileBase64; lock_error=$lastLockError"
exit 1
