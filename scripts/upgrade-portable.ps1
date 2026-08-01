param(
    [Parameter(Mandatory = $true)]
    [string]$TargetDir,
    [switch]$StartAfterUpgrade,
    [switch]$DeleteSourceAfterUpgrade,
    [string]$RegistryPrefix = "HKCU:\Software"
)

$ErrorActionPreference = "Stop"

$sourceDir = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$targetDirFull = [System.IO.Path]::GetFullPath($TargetDir)

function Get-DirectoryPrefix {
    param([string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path).TrimEnd("\", "/")
    return $full + [System.IO.Path]::DirectorySeparatorChar
}

if (-not (Test-Path -LiteralPath (Join-Path $sourceDir "portable") -PathType Leaf) -or
    -not (Test-Path -LiteralPath (Join-Path $sourceDir "HLSDownloader.exe") -PathType Leaf)) {
    throw "Run this script from the newly extracted HLS Downloader portable archive."
}
if (-not (Test-Path -LiteralPath $targetDirFull -PathType Container) -or
    -not (Test-Path -LiteralPath (Join-Path $targetDirFull "HLSDownloader.exe") -PathType Leaf) -or
    -not (Test-Path -LiteralPath (Join-Path $targetDirFull "portable") -PathType Leaf)) {
    throw "TargetDir is not an existing HLS Downloader portable folder: $targetDirFull"
}
if ([string]::Equals($sourceDir.TrimEnd('\'), $targetDirFull.TrimEnd('\'), [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Extract the new archive to a different folder before upgrading the existing portable copy."
}
if ((Get-DirectoryPrefix $sourceDir).StartsWith((Get-DirectoryPrefix $targetDirFull), [System.StringComparison]::OrdinalIgnoreCase) -or
    (Get-DirectoryPrefix $targetDirFull).StartsWith((Get-DirectoryPrefix $sourceDir), [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "The new archive and existing portable folder must not be nested inside each other."
}

$targetParent = [System.IO.Path]::GetDirectoryName($targetDirFull.TrimEnd('\', '/'))
$targetName = [System.IO.Path]::GetFileName($targetDirFull.TrimEnd('\', '/'))
$stageDir = Join-Path $targetParent ".$targetName.hls-upgrade-new"
$backupDir = Join-Path $targetParent ".$targetName.hls-upgrade-backup"
foreach ($transactionPath in @($stageDir, $backupDir)) {
    if (-not [string]::Equals(
        [System.IO.Path]::GetDirectoryName([System.IO.Path]::GetFullPath($transactionPath)),
        $targetParent,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Refusing to use an upgrade transaction path outside the target parent: $transactionPath"
    }
}

# Recover a directory swap interrupted after the old folder was moved but
# before the fully prepared replacement took its place.
if (-not (Test-Path -LiteralPath $targetDirFull) -and (Test-Path -LiteralPath $backupDir -PathType Container)) {
    Move-Item -LiteralPath $backupDir -Destination $targetDirFull
}
if (-not (Test-Path -LiteralPath $targetDirFull -PathType Container)) {
    throw "The existing portable folder could not be recovered: $targetDirFull"
}
if (Test-Path -LiteralPath $backupDir) {
    # Both folders can coexist only after an atomic swap completed and cleanup
    # was interrupted. The new target was assembled in full before that swap.
    Remove-Item -LiteralPath $backupDir -Recurse -Force
}
if (Test-Path -LiteralPath $stageDir) {
    Remove-Item -LiteralPath $stageDir -Recurse -Force
}

$newRegistrationSource = Join-Path $sourceDir "scripts\register-native-host.ps1"
$shutdownScript = Join-Path $sourceDir "scripts\shutdown-running.ps1"
if (-not (Test-Path -LiteralPath $newRegistrationSource -PathType Leaf) -or
    -not (Test-Path -LiteralPath $shutdownScript -PathType Leaf)) {
    throw "The new portable archive is missing upgrade support scripts."
}

$oldRegistration = Join-Path $targetDirFull "scripts\register-native-host.ps1"
$preservedNames = @(
    "config.json",
    "data.db",
    "data.db-shm",
    "data.db-wal",
    "downloads",
    ".tasks",
    ".webview"
)

$oldUnregistered = $false
$oldMoved = $false
$newMoved = $false
try {
    if (Test-Path -LiteralPath $oldRegistration -PathType Leaf) {
        & $oldRegistration -Unregister -RegistryPrefix $RegistryPrefix
        if (-not $?) { throw "The old browser integration could not be disconnected." }
        $oldUnregistered = $true
    }

    & $shutdownScript -InstallDir $targetDirFull -TimeoutSeconds 20 -IncludeNativeHost
    if ($LASTEXITCODE -ne 0) {
        throw "The running portable application could not be closed. Close it and all browser extension connections, then try again."
    }

    # Build a complete replacement beside the target. Runtime state is copied
    # from the old folder only after all application processes are closed.
    New-Item -ItemType Directory -Path $stageDir | Out-Null
    Get-ChildItem -LiteralPath $sourceDir -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $stageDir $_.Name) -Recurse -Force
    }
    foreach ($name in $preservedNames) {
        $existing = Join-Path $targetDirFull $name
        if (-not (Test-Path -LiteralPath $existing)) { continue }
        $staged = Join-Path $stageDir $name
        if (Test-Path -LiteralPath $staged) {
            Remove-Item -LiteralPath $staged -Recurse -Force
        }
        Copy-Item -LiteralPath $existing -Destination $staged -Recurse -Force
    }
    foreach ($required in @("HLSDownloader.exe", "HLSDownloaderCore.exe", "portable", "scripts\register-native-host.ps1")) {
        if (-not (Test-Path -LiteralPath (Join-Path $stageDir $required))) {
            throw "The prepared upgrade is missing: $required"
        }
    }

    # Same-parent directory moves are atomic on NTFS. No partially copied
    # program tree ever becomes the runnable target.
    Move-Item -LiteralPath $targetDirFull -Destination $backupDir
    $oldMoved = $true
    Move-Item -LiteralPath $stageDir -Destination $targetDirFull
    $newMoved = $true

    $newRegistration = Join-Path $targetDirFull "scripts\register-native-host.ps1"
    & $newRegistration -RegistryPrefix $RegistryPrefix
    if (-not $? -or $LASTEXITCODE -ne 0) {
        throw "The application was upgraded, but browser integration could not be registered."
    }

    Remove-Item -LiteralPath $backupDir -Recurse -Force
    $oldMoved = $false
} catch {
    $failure = $_
    if ($newMoved -and (Test-Path -LiteralPath $targetDirFull)) {
        Remove-Item -LiteralPath $targetDirFull -Recurse -Force -ErrorAction SilentlyContinue
    }
    if ($oldMoved -and (Test-Path -LiteralPath $backupDir -PathType Container)) {
        Move-Item -LiteralPath $backupDir -Destination $targetDirFull
        $oldMoved = $false
    }
    if ($oldUnregistered -and (Test-Path -LiteralPath $oldRegistration -PathType Leaf)) {
        & $oldRegistration -RegistryPrefix $RegistryPrefix
    }
    throw $failure
} finally {
    if (Test-Path -LiteralPath $stageDir) {
        Remove-Item -LiteralPath $stageDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "Portable HLS Downloader upgraded successfully: $targetDirFull" -ForegroundColor Green
if ($StartAfterUpgrade) {
    Start-Process -FilePath (Join-Path $targetDirFull "HLSDownloader.exe") -WorkingDirectory $targetDirFull
}
if ($DeleteSourceAfterUpgrade) {
    # The script has already been parsed into this PowerShell process and the
    # working directory is outside sourceDir, so the verified staging tree can
    # be removed after a successful atomic swap.
    Remove-Item -LiteralPath $sourceDir -Recurse -Force -ErrorAction SilentlyContinue
}
