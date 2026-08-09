param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath
)

$ErrorActionPreference = "Stop"
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$installer = if ([IO.Path]::IsPathRooted($InstallerPath)) {
    [IO.Path]::GetFullPath($InstallerPath)
} else {
    [IO.Path]::GetFullPath((Join-Path $root $InstallerPath))
}
$smokeRoot = [IO.Path]::GetFullPath((Join-Path $root "build\installer-smoke"))
$installDir = Join-Path $smokeRoot "app"
$registryPrefix = "HKCU:\Software\HLSDownloaderInstallerSmoke"

if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    throw "Installer not found: $installer"
}
$packageVersion = (Get-Item -LiteralPath $installer).VersionInfo.ProductVersion
if ($packageVersion -notmatch '^\d+(?:\.\d+){1,3}$') {
    throw "Installer has an invalid product version: $packageVersion"
}
if ((Split-Path -Parent $smokeRoot) -ne (Join-Path $root "build")) {
    throw "Refusing to use an unexpected smoke root: $smokeRoot"
}

$officialRegistryKeys = @(
    @{ Path = "HKCU:\Software\Google\Chrome\NativeMessagingHosts\com.ciaooo55.hls_downloader"; Names = @("(default)") },
    @{ Path = "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\com.ciaooo55.hls_downloader"; Names = @("(default)") },
    @{ Path = "HKCU:\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.ciaooo55.hls_downloader"; Names = @("(default)") },
    @{ Path = "HKCU:\Software\Chromium\NativeMessagingHosts\com.ciaooo55.hls_downloader"; Names = @("(default)") },
    @{ Path = "HKCU:\Software\Vivaldi\NativeMessagingHosts\com.ciaooo55.hls_downloader"; Names = @("(default)") },
    @{ Path = "HKCU:\Software\Opera Software\NativeMessagingHosts\com.ciaooo55.hls_downloader"; Names = @("(default)") },
    @{ Path = "HKCU:\Software\Mozilla\NativeMessagingHosts\com.ciaooo55.hls_downloader"; Names = @("(default)") },
    @{ Path = "HKCU:\Software\HLS Downloader"; Names = @("InstallDir", "PreviousTorrentProgId") },
    @{ Path = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\HLS Downloader"; Names = @("DisplayVersion", "InstallLocation") },
    @{ Path = "HKCU:\Software\Classes\.torrent"; Names = @("(default)") }
)

function Get-RegistrySnapshot {
    $rows = foreach ($entry in $officialRegistryKeys) {
        $key = Get-Item -LiteralPath $entry.Path -ErrorAction SilentlyContinue
        foreach ($name in $entry.Names) {
            $value = $null
            if ($key) {
                $valueName = if ($name -eq "(default)") { "" } else { $name }
                $value = $key.GetValue($valueName, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            }
            [ordered]@{
                path = $entry.Path
                name = $name
                key_exists = [bool]$key
                value = $value
            }
        }
    }
    return ($rows | ConvertTo-Json -Depth 4 -Compress)
}

function Get-ApplicationProcesses([switch]$OnlySmoke) {
    $items = @(Get-Process -Name "HLSDownloader", "HLSDownloaderCore", "HLSDownloaderNativeHost*" -ErrorAction SilentlyContinue)
    return @($items | Where-Object {
        try {
            $path = [IO.Path]::GetFullPath($_.Path)
            $isSmoke = $path.StartsWith($smokeRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)
            if ($OnlySmoke) { $isSmoke } else { -not $isSmoke }
        } catch {
            -not $OnlySmoke
        }
    })
}

function Invoke-Installer([string]$Path) {
    $process = Start-Process -FilePath $Path -ArgumentList @(
        "/S",
        "/BUILD-SMOKE=1",
        "/D=$installDir"
    ) -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(90000)) {
        $process | Stop-Process -Force -ErrorAction SilentlyContinue
        throw "Installer did not finish within the bounded 90-second smoke window"
    }
    $process.Refresh()
    if ($process.ExitCode -ne 0) {
        throw "Installer returned exit code $($process.ExitCode)"
    }
}

function Write-JsonNoBom([string]$Path, [object]$Value) {
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($Path, ($Value | ConvertTo-Json -Depth 12), $utf8NoBom)
}

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Mutate-ExecutableDosStub([string]$Path) {
    # Change one ignored DOS-stub byte without changing the file size or the
    # PyInstaller archive footer.  Appending an ordinary PE overlay is not a
    # valid fixture here because PyInstaller locates its package cookie at EOF.
    # The result remains runnable but has a different SHA-256, which proves the
    # cover installer actually restored the packaged Core.
    $stream = [IO.File]::Open(
        $Path,
        [IO.FileMode]::Open,
        [IO.FileAccess]::ReadWrite,
        [IO.FileShare]::None
    )
    try {
        $pePointer = New-Object byte[] 4
        $null = $stream.Seek(0x3c, [IO.SeekOrigin]::Begin)
        if ($stream.Read($pePointer, 0, $pePointer.Length) -ne $pePointer.Length) {
            throw "Core executable is too small to contain a PE header"
        }
        $peOffset = [BitConverter]::ToInt32($pePointer, 0)
        if ($peOffset -le 0x41) {
            throw "Core executable has no safe DOS-stub byte for the stale fixture"
        }
        $null = $stream.Seek(0x40, [IO.SeekOrigin]::Begin)
        $original = $stream.ReadByte()
        if ($original -lt 0) { throw "Core executable DOS stub is truncated" }
        $null = $stream.Seek(0x40, [IO.SeekOrigin]::Begin)
        $stream.WriteByte([byte]($original -bxor 1))
        $stream.Flush($true)
    } finally {
        $stream.Dispose()
    }
}

function Start-SmokeApplication {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $port = ([Net.IPEndPoint]$listener.LocalEndpoint).Port
    $listener.Stop()

    $configPath = Join-Path $installDir "config.json"
    $config = Get-Content -LiteralPath (Join-Path $installDir "config.default.json") -Raw -Encoding UTF8 | ConvertFrom-Json
    $config.port = $port
    $config.default_referer = "https://installer-smoke.invalid/preserved"
    Write-JsonNoBom $configPath $config
    Set-Content -LiteralPath (Join-Path $installDir "portable") -Value "" -Encoding ASCII

    $previousSmoke = $env:HLS_DOWNLOADER_BUILD_SMOKE
    try {
        $env:HLS_DOWNLOADER_BUILD_SMOKE = "1"
        $desktop = Start-Process -FilePath (Join-Path $installDir "HLSDownloader.exe") -ArgumentList "--background" -WorkingDirectory $installDir -WindowStyle Hidden -PassThru
    } finally {
        $env:HLS_DOWNLOADER_BUILD_SMOKE = $previousSmoke
    }

    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 250
        $running = @(Get-ApplicationProcesses -OnlySmoke)
        if ($running.Name -contains "HLSDownloader" -and $running.Name -contains "HLSDownloaderCore") {
            return $desktop.Id
        }
    } while ([DateTime]::UtcNow -lt $deadline)
    $desktop.Refresh()
    $desktopState = if ($desktop.HasExited) { "exited with code $($desktop.ExitCode)" } else { "still running" }
    $processState = @($running | ForEach-Object { "$($_.Name):$($_.Id):$($_.Path)" }) -join "; "
    $coreErrors = ""
    $coreErrorPath = Join-Path $installDir "core-error.log"
    if (Test-Path -LiteralPath $coreErrorPath) {
        $coreErrors = (Get-Content -LiteralPath $coreErrorPath -Tail 20 -ErrorAction SilentlyContinue) -join " | "
    }
    throw "The isolated installed application did not start its desktop and Core processes; desktop $desktopState; observed [$processState]; core-error [$coreErrors]"
}

function Assert-OfficialState(
    [string]$ExpectedRegistry,
    [int[]]$ExpectedProcessIds = @()
) {
    $actualRegistry = Get-RegistrySnapshot
    if ($actualRegistry -ne $ExpectedRegistry) {
        throw "Installer smoke changed production browser, application, uninstall, or torrent registry state"
    }
    $actualIds = @(
        Get-ApplicationProcesses | ForEach-Object { [int]$_.Id }
    )
    foreach ($id in $ExpectedProcessIds) {
        if ($id -notin $actualIds) {
            throw "Installer smoke stopped an existing application process: $id"
        }
    }
}

$officialRegistryBefore = Get-RegistrySnapshot
$officialProcessIdsBefore = @(
    Get-ApplicationProcesses | ForEach-Object { [int]$_.Id }
)
$result = $null
try {
    if (Test-Path -LiteralPath $smokeRoot) {
        Remove-Item -LiteralPath $smokeRoot -Recurse -Force
    }
    Remove-Item -LiteralPath $registryPrefix -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $smokeRoot | Out-Null

    Invoke-Installer $installer
    foreach ($required in @(
        "HLSDownloader.exe",
        "HLSDownloaderCore.exe",
        "Uninstall.exe",
        "config.default.json",
        "scripts\register-native-host.ps1",
        "scripts\shutdown-running.ps1",
        "native-host\versions\HLSDownloaderNativeHost-$packageVersion.exe"
    )) {
        if (-not (Test-Path -LiteralPath (Join-Path $installDir $required))) {
            throw "Initial install is missing: $required"
        }
    }
    $desktopPath = Join-Path $installDir "HLSDownloader.exe"
    $corePath = Join-Path $installDir "HLSDownloaderCore.exe"
    $expectedDesktopHash = Get-Sha256 $desktopPath
    $expectedCoreHash = Get-Sha256 $corePath
    if ((Get-Item -LiteralPath $desktopPath).VersionInfo.ProductVersion -ne $packageVersion) {
        throw "Installed desktop version does not match the package version"
    }
    if ((Get-Item -LiteralPath $corePath).VersionInfo.ProductVersion -ne $packageVersion) {
        throw "Installed Core version does not match the package version"
    }
    $versionedIcon = Join-Path $installDir "assets\app-icon-$packageVersion.ico"
    if (-not (Test-Path -LiteralPath $versionedIcon -PathType Leaf)) {
        throw "Initial install is missing the versioned shell icon"
    }
    $expectedNativeHost = [IO.Path]::GetFullPath(
        (Join-Path $installDir "native-host\versions\HLSDownloaderNativeHost-$packageVersion.exe")
    )
    if ((Get-Item -LiteralPath $expectedNativeHost).VersionInfo.ProductVersion -ne $packageVersion) {
        throw "Installed Native Host version does not match the package version"
    }
    foreach ($browserKey in @(
        "Google\Chrome",
        "Microsoft\Edge",
        "BraveSoftware\Brave-Browser",
        "Chromium",
        "Vivaldi",
        "Opera Software",
        "Mozilla"
    )) {
        $key = Join-Path $registryPrefix "$browserKey\NativeMessagingHosts\com.ciaooo55.hls_downloader"
        $manifest = (Get-Item -LiteralPath $key -ErrorAction Stop).GetValue("")
        if (-not $manifest -or -not [IO.Path]::GetFullPath($manifest).StartsWith($installDir, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Isolated Native Messaging registration points outside the smoke install: $browserKey"
        }
        $registeredManifest = Get-Content -LiteralPath $manifest -Raw -Encoding UTF8 | ConvertFrom-Json
        if (-not [string]::Equals(
            [IO.Path]::GetFullPath([string]$registeredManifest.path),
            $expectedNativeHost,
            [StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Isolated Native Messaging registration did not select the package host: $browserKey"
        }
    }
    Assert-OfficialState $officialRegistryBefore $officialProcessIdsBefore

    Copy-Item -LiteralPath (Join-Path $installDir "config.default.json") -Destination (Join-Path $installDir "config.json")
    $preservedConfig = Get-Content -LiteralPath (Join-Path $installDir "config.json") -Raw -Encoding UTF8 | ConvertFrom-Json
    $preservedConfig.default_referer = "https://installer-smoke.invalid/preserved"
    Write-JsonNoBom (Join-Path $installDir "config.json") $preservedConfig
    Set-Content -LiteralPath (Join-Path $installDir "data.db") -Value "installer upgrade database sentinel" -Encoding UTF8
    New-Item -ItemType Directory -Path (Join-Path $installDir "downloads"), (Join-Path $installDir "app"), (Join-Path $installDir "runtime") | Out-Null
    Set-Content -LiteralPath (Join-Path $installDir "downloads\keep.txt") -Value "installer upgrade download sentinel" -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $installDir "app\stale.txt") -Value "legacy app" -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $installDir "runtime\stale.txt") -Value "legacy runtime" -Encoding UTF8

    Invoke-Installer $installer
    if ((Get-Sha256 $desktopPath) -ne $expectedDesktopHash -or (Get-Sha256 $corePath) -ne $expectedCoreHash) {
        throw "Cover install did not restore the packaged desktop/Core executables"
    }
    if ((Get-Content -LiteralPath (Join-Path $installDir "config.json") -Raw) -notmatch "installer-smoke.invalid/preserved") {
        throw "Cover install did not preserve config.json"
    }
    if ((Get-Content -LiteralPath (Join-Path $installDir "data.db") -Raw) -notmatch "database sentinel") {
        throw "Cover install did not preserve data.db"
    }
    if ((Get-Content -LiteralPath (Join-Path $installDir "downloads\keep.txt") -Raw) -notmatch "download sentinel") {
        throw "Cover install did not preserve downloads"
    }
    if ((Test-Path -LiteralPath (Join-Path $installDir "app")) -or (Test-Path -LiteralPath (Join-Path $installDir "runtime"))) {
        throw "Cover install left legacy application/runtime directories behind"
    }

    Remove-Item -LiteralPath (Join-Path $installDir "data.db") -Force
    $directShutdownDesktopId = Start-SmokeApplication
    & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass `
        -File (Join-Path $installDir "scripts\shutdown-running.ps1") `
        -InstallDir $installDir `
        -TimeoutSeconds 12 `
        -IncludeNativeHost
    if ($LASTEXITCODE -ne 0) {
        throw "Packaged shutdown helper returned exit code $LASTEXITCODE"
    }
    if (Get-Process -Id $directShutdownDesktopId -ErrorAction SilentlyContinue) {
        throw "Packaged shutdown helper did not close the running desktop process"
    }
    $processDeadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $remaining = @(Get-ApplicationProcesses -OnlySmoke)
        if (-not $remaining.Count) { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $processDeadline)
    if ($remaining.Count) {
        $details = ($remaining | ForEach-Object { "$($_.Name):$($_.Id):$($_.Path)" }) -join "; "
        throw "Packaged shutdown helper left isolated processes running: $details"
    }

    Mutate-ExecutableDosStub $corePath
    if ((Get-Sha256 $corePath) -eq $expectedCoreHash) {
        throw "Installer smoke could not create a stale Core fixture"
    }
    $upgradeDesktopId = Start-SmokeApplication
    Invoke-Installer $installer
    if (Get-Process -Id $upgradeDesktopId -ErrorAction SilentlyContinue) {
        $shutdownLogPath = Join-Path $installDir "installer-smoke-shutdown.log"
        $shutdownLog = if (Test-Path -LiteralPath $shutdownLogPath) {
            (Get-Content -LiteralPath $shutdownLogPath -Raw -ErrorAction SilentlyContinue).Trim()
        } else {
            "missing"
        }
        throw "Cover install did not close the running desktop process; NSIS shutdown [$shutdownLog]"
    }
    $processDeadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $remaining = @(Get-ApplicationProcesses -OnlySmoke)
        if (-not $remaining.Count) { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $processDeadline)
    if ($remaining.Count) {
        $details = ($remaining | ForEach-Object { "$($_.Name):$($_.Id):$($_.Path)" }) -join "; "
        throw "Cover install left isolated desktop/Core/Native Host processes running: $details"
    }
    if ((Get-Sha256 $desktopPath) -ne $expectedDesktopHash -or (Get-Sha256 $corePath) -ne $expectedCoreHash) {
        throw "Running cover install left a stale desktop/Core executable behind"
    }
    if ((Get-Content -LiteralPath (Join-Path $installDir "config.json") -Raw) -notmatch "installer-smoke.invalid/preserved") {
        throw "Running cover install did not preserve config.json"
    }
    Assert-OfficialState $officialRegistryBefore $officialProcessIdsBefore

    $uninstallDesktopId = Start-SmokeApplication
    $uninstaller = Join-Path $installDir "Uninstall.exe"
    $shutdownLogPath = Join-Path $installDir "installer-smoke-shutdown.log"
    Remove-Item -LiteralPath $shutdownLogPath -Force -ErrorAction SilentlyContinue
    $uninstall = Start-Process -FilePath $uninstaller -ArgumentList @("/S", "/BUILD-SMOKE=1") -WindowStyle Hidden -Wait -PassThru
    if ($uninstall.ExitCode -ne 0) {
        throw "Uninstaller returned exit code $($uninstall.ExitCode)"
    }
    # NSIS first launches a temporary uninstaller and lets the original process
    # exit. Start-Process -Wait therefore does not reliably mean the real
    # uninstall section has finished. Wait for observable completion instead
    # of reporting a healthy asynchronous uninstall as a shutdown failure.
    $processDeadline = [DateTime]::UtcNow.AddSeconds(60)
    do {
        $remaining = @(Get-ApplicationProcesses -OnlySmoke)
        $shutdownLog = if (Test-Path -LiteralPath $shutdownLogPath) {
            (Get-Content -LiteralPath $shutdownLogPath -Raw -Encoding Default -ErrorAction SilentlyContinue).Trim()
        } else {
            ""
        }
        $uninstallFinished = (
            -not $remaining.Count -and
            -not (Test-Path -LiteralPath $uninstaller) -and
            $shutdownLog -match '(?m)^exit=0\r?$'
        )
        if ($uninstallFinished) { break }
        if ($shutdownLog -match '(?m)^exit=(?!0\r?$)') { break }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $processDeadline)
    if (Get-Process -Id $uninstallDesktopId -ErrorAction SilentlyContinue) {
        $diagnostic = if ($shutdownLog) { $shutdownLog } else { "missing" }
        throw "Uninstaller did not close the running desktop process; NSIS shutdown [$diagnostic]"
    }
    if ($remaining.Count) {
        $details = ($remaining | ForEach-Object { "$($_.Name):$($_.Id):$($_.Path)" }) -join "; "
        throw "Uninstaller left isolated application processes running: $details"
    }
    if (-not $uninstallFinished) {
        $diagnostic = if ($shutdownLog) { $shutdownLog } else { "missing" }
        throw "Uninstaller did not finish removing its application image; NSIS shutdown [$diagnostic]"
    }
    foreach ($removed in @("HLSDownloader.exe", "HLSDownloaderCore.exe", "Uninstall.exe", "frontend", "_internal", "native-host")) {
        if (Test-Path -LiteralPath (Join-Path $installDir $removed)) {
            throw "Uninstaller left application content behind: $removed"
        }
    }
    if (-not (Test-Path -LiteralPath (Join-Path $installDir "downloads\keep.txt"))) {
        throw "Silent uninstall unexpectedly deleted completed downloads"
    }
    Assert-OfficialState $officialRegistryBefore $officialProcessIdsBefore

    $result = [PSCustomObject]@{
        InitialInstall = $true
        CoverInstallPreservedConfig = $true
        CoverInstallPreservedDatabase = $true
        CoverInstallPreservedDownloads = $true
        CoverInstallRemovedLegacyRuntime = $true
        CoverInstallReplacedExecutables = $true
        RunningCoverInstallReplacedStaleCore = $true
        VersionedShellIcon = $true
        ExactNativeHostRegistered = $true
        ExecutableVersionsMatchPackage = $true
        PackagedShutdownHelperClosedRunningApp = $true
        CoverInstallClosedRunningApp = $true
        UninstallClosedRunningApp = $true
        SilentUninstallPreservedDownloads = $true
        BrowserRegistryIsolated = $true
        OfficialProcessesPreserved = $true
    }
} finally {
    @(Get-ApplicationProcesses -OnlySmoke) | Stop-Process -Force -ErrorAction SilentlyContinue
    if ($env:HLS_INSTALLER_SMOKE_KEEP -eq "1") {
        Write-Warning "Keeping installer smoke artifacts for diagnosis: $smokeRoot"
    } else {
        Remove-Item -LiteralPath $registryPrefix -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $smokeRoot) {
            Remove-Item -LiteralPath $smokeRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

$result
