param(
    [int]$TimeoutSeconds = 12,
    [string]$InstallDir = "",
    [switch]$IncludeNativeHost,
    [switch]$RequireCheckpoint
)

$ErrorActionPreference = 'Stop'
if ([String]::IsNullOrWhiteSpace($InstallDir)) { throw 'Shutdown requires an install directory.' }
$InstallDir = (Resolve-Path -LiteralPath $InstallDir).ProviderPath
if (-not [IO.Directory]::Exists($InstallDir) -or $InstallDir.TrimEnd('\') -eq [IO.Path]::GetPathRoot($InstallDir).TrimEnd('\')) {
    throw "Invalid install directory: $InstallDir"
}
$installRoot = $InstallDir.TrimEnd('\') + '\'
$script:shutdownStarted = [DateTime]::UtcNow

function Write-ShutdownStage {
    param([string]$Stage)
    Write-Output ("shutdown stage={0} elapsed_ms={1}" -f $Stage, [int]([DateTime]::UtcNow - $script:shutdownStarted).TotalMilliseconds)
}

Write-ShutdownStage "entry user=$env:USERNAME pid=$PID"

function Get-ProcessExecutablePath {
    param([System.Diagnostics.Process]$Process)

    $resolved = ""
    try {
        if ($Process.HasExited) { return "" }
        $resolved = [string]$Process.Path
    } catch {
    }
    # A 32-bit caller cannot always read MainModule/Path for a 64-bit target.
    # CIM remains architecture-neutral and keeps shutdown scoped to InstallDir.
    # Bound this fallback: repeatedly starting an unbounded WMI query
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
    return $resolved
}

function Get-TargetProcesses {
    param([string[]]$Names)

    $processes = @(Get-Process -Name $Names -ErrorAction SilentlyContinue)
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
Write-ShutdownStage "processes count=$($targetRunningAtStart.Count)"
$overallDeadline = [DateTime]::UtcNow.AddSeconds([Math]::Max(3, $TimeoutSeconds))
$engineNames = @('HLSDownloaderEngine', 'hls-downloader-engine')
$gracefulCoreExit = "not-running"
# 先关闭会重新连接 Core 的工作台及桥接进程，防止停机期间再次启动引擎。
if ($RequireCheckpoint) {
    Write-ShutdownStage 'close-clients'
    foreach ($desktop in @(Get-TargetProcesses @('HLSDownloader'))) {
        $desktop.CloseMainWindow() | Out-Null
    }
    Start-Sleep -Milliseconds 300
    foreach ($desktop in @(Get-TargetProcesses @('HLSDownloader'))) {
        # 不使用 /T：Core 可能是其子进程，必须先单独保存续传状态。
        $desktop | Stop-Process -Force -ErrorAction SilentlyContinue
    }
    Get-TargetProcesses @('HLSDownloaderNativeHost*') | Stop-Process -Force -ErrorAction SilentlyContinue
}
function Read-CheckpointBytes($Pipe, [byte[]]$Buffer, [DateTime]$Deadline) {
    $offset = 0
    while ($offset -lt $Buffer.Length) {
        $read = $Pipe.ReadAsync($Buffer, $offset, $Buffer.Length - $offset)
        $remaining = [Math]::Max(0, ($Deadline - [DateTime]::UtcNow).TotalMilliseconds)
        if (-not $read.Wait([int]$remaining) -or $read.Result -eq 0) { throw 'Incomplete checkpoint response.' }
        $offset += $read.Result
    }
}

function Request-Checkpoint([Diagnostics.Process]$Engine) {
    # Send on the handle whose server PID was checked. Engine --shutdown
    # opens a new connection and can select a different installation.
    if (-not ('HlsShutdown.Pipe' -as [type])) {
        Add-Type -Namespace HlsShutdown -Name Pipe -MemberDefinition '
            [System.Runtime.InteropServices.DllImport("kernel32.dll")]
            public static extern bool GetNamedPipeServerProcessId(Microsoft.Win32.SafeHandles.SafePipeHandle pipe, out uint pid);'
    }
    $names = @("HLSDownloader.v7.maintenance.$($Engine.Id)")
    if ($env:HLS_V7_PIPE -and $env:HLS_V7_PIPE.StartsWith('\\.\pipe\', [StringComparison]::OrdinalIgnoreCase)) {
        $names += $env:HLS_V7_PIPE.Substring(9)
    }
    $names += 'HLSDownloader.v7'
    foreach ($name in $names) {
        if ($Engine.HasExited) { return }
        $pipe = [IO.Pipes.NamedPipeClientStream]::new('.', $name, [IO.Pipes.PipeDirection]::InOut,
            [IO.Pipes.PipeOptions]::Asynchronous, [Security.Principal.TokenImpersonationLevel]::Identification)
        try {
            try { $pipe.Connect(250) } catch [TimeoutException] { continue }
            $serverId = [uint32]0
            if (-not [HlsShutdown.Pipe]::GetNamedPipeServerProcessId($pipe.SafePipeHandle, [ref]$serverId) -or
                $serverId -ne $Engine.Id -or $Engine.HasExited) { continue }
            $body = [Text.Encoding]::UTF8.GetBytes('{"type":"command","request_id":1,"command":{"kind":"shutdown"}}')
            [byte[]]$frame = [BitConverter]::GetBytes([int]$body.Length) + $body
            $pipe.Write($frame, 0, $frame.Length)
            $deadline = [DateTime]::UtcNow.AddSeconds(12)
            $header = New-Object byte[] 4
            Read-CheckpointBytes $pipe $header $deadline
            $length = [BitConverter]::ToInt32($header, 0)
            if ($length -lt 1 -or $length -gt 4MB) { throw 'Invalid checkpoint response length.' }
            $data = New-Object byte[] $length
            Read-CheckpointBytes $pipe $data $deadline
            $reply = [Text.Encoding]::UTF8.GetString($data) | ConvertFrom-Json
            if ($reply.type -ne 'events' -or $reply.request_id -ne 1 -or
                @($reply.events | Where-Object { $_.event.kind -eq 'error' }).Count -gt 0) {
                throw 'Core rejected the checkpoint request.'
            }
            return
        } finally {
            $pipe.Dispose()
        }
    }
    throw "No matching checkpoint pipe for Core $($Engine.Id)."
}

foreach ($engine in @(Get-TargetProcesses $engineNames)) {
    Write-ShutdownStage 'checkpoint-start'
    try {
        Request-Checkpoint $engine
        $gracefulCoreExit = '0'
    } catch {
        $gracefulCoreExit = $_.Exception.Message
        if ($RequireCheckpoint -and -not $engine.HasExited) { throw }
    }
    Write-ShutdownStage "checkpoint-finished code=$gracefulCoreExit"
}
if ($RequireCheckpoint) {
    # A checkpointed Core exits itself. Never force-stop a newly started Core.
    foreach ($engine in @(Get-TargetProcesses $engineNames)) {
        if (-not $engine.WaitForExit(1000)) { throw 'Core is still running after checkpoint.' }
    }
    $targetProcessNames = @($targetProcessNames | Where-Object { $_ -notin $engineNames })
}

$gracefulDeadline = [DateTime]::UtcNow.AddSeconds([Math]::Min(4, [Math]::Max(1, $TimeoutSeconds)))
if ($gracefulDeadline -gt $overallDeadline) { $gracefulDeadline = $overallDeadline }
while ([DateTime]::UtcNow -lt $gracefulDeadline) {
    if (-not (Get-TargetProcesses @("HLSDownloader"))) { break }
    Start-Sleep -Milliseconds 200
}

foreach ($desktop in @(Get-TargetProcesses @("HLSDownloader"))) {
    $desktop | Stop-Process -Force -ErrorAction SilentlyContinue
}
Get-TargetProcesses @("HLSDownloaderCore") | Stop-Process -Force -ErrorAction SilentlyContinue

if ($IncludeNativeHost) {
    # 卸载和升级预停机都必须关闭桥接进程，防止它在事务期间再次启动 Core。
    Get-TargetProcesses @("HLSDownloaderNativeHost") | Stop-Process -Force -ErrorAction SilentlyContinue
}

function Test-ApplicationFilesWritable {
    $script:lastLockedFile = ""
    $script:lastLockError = ""
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
    if ($RequireCheckpoint -and @(Get-TargetProcesses $engineNames).Count -gt 0) { throw 'Core restarted after checkpoint.' }
    if (-not $running -and (Test-ApplicationFilesWritable)) { Write-ShutdownStage 'finished'; exit 0 }
    if ($running) {
        $running | Stop-Process -Force -ErrorAction SilentlyContinue
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
