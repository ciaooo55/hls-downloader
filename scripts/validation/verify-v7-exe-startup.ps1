[CmdletBinding()]
param(
    [string]$CandidateManifestPath = '',
    [string]$ProbeInstallDir = '',
    [string]$ProbeShortcut = '',
    [string]$ProbeReport = ''
)

$ErrorActionPreference = 'Stop'
# Installation and visible-window checks run only on a disposable hosted runner.
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'EXE startup verification requires an isolated GitHub Actions runner.' }
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$utf8 = New-Object Text.UTF8Encoding($false)

if ($ProbeInstallDir) {
    $probe = [ordered]@{ passed = $false; launch = 'installed-start-menu-shortcut'; session_id = (Get-Process -Id $PID).SessionId }
    try {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class HlsStartupWindow {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageTimeoutW(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam, uint flags, uint timeout, out UIntPtr result);
}
'@
        Start-Process -FilePath $ProbeShortcut | Out-Null
        $deadline = (Get-Date).AddSeconds(45)
        do {
            $presenter = @(Get-Process -Name HLSDownloaderPresenter -ErrorAction SilentlyContinue | Where-Object {
                $_.Path -and $_.Path.StartsWith($ProbeInstallDir.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase) -and
                $_.MainWindowTitle -like 'HLS Downloader*' -and [HlsStartupWindow]::IsWindowVisible($_.MainWindowHandle)
            }) | Select-Object -First 1
            if ($presenter) { break }
            Start-Sleep -Milliseconds 200
        } while ((Get-Date) -lt $deadline)
        if (-not $presenter) { throw 'Installed shortcut did not open a visible workbench within 45 seconds.' }
        $rect = New-Object HlsStartupWindow+Rect
        if (-not [HlsStartupWindow]::GetWindowRect($presenter.MainWindowHandle, [ref]$rect)) { throw 'Cannot inspect workbench bounds.' }
        if (($rect.Right - $rect.Left) -lt 400 -or ($rect.Bottom - $rect.Top) -lt 250) { throw 'Workbench has invalid visible bounds.' }
        $reply = [UIntPtr]::Zero
        if ([HlsStartupWindow]::SendMessageTimeoutW($presenter.MainWindowHandle, 0, [UIntPtr]::Zero, [IntPtr]::Zero, 2, 3000, [ref]$reply) -eq [IntPtr]::Zero) { throw 'Visible workbench is unresponsive.' }
        $core = @(Get-Process -Name HLSDownloaderEngine -ErrorAction SilentlyContinue | Where-Object {
            $_.Path -and $_.Path.StartsWith($ProbeInstallDir.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)
        })
        if ($core.Count -ne 1) { throw "Expected one installed Core, found $($core.Count)." }
        $probe.passed = $true
        $probe.presenter_pid = $presenter.Id
        $probe.engine_pid = $core[0].Id
        $probe.title = $presenter.MainWindowTitle
        $probe.window = [ordered]@{ width = $rect.Right - $rect.Left; height = $rect.Bottom - $rect.Top; responsive = $true }
    } catch { $probe.error = $_.Exception.Message }
    [IO.File]::WriteAllText(($ProbeReport + '.pending'), ($probe | ConvertTo-Json -Depth 4), $utf8)
    Move-Item -LiteralPath ($ProbeReport + '.pending') -Destination $ProbeReport
    if (-not $probe.passed) { exit 1 }
    exit 0
}

. (Join-Path $PSScriptRoot 'V7MsiInspection.ps1')
$artifacts = Join-Path $repo 'artifacts\v7-productization\exe-startup'
$fixture = Join-Path $repo '.tool-cache\test-tmp\exe-startup'
if (Test-Path -LiteralPath $fixture) { throw "EXE startup requires a fresh fixture: $fixture" }
New-Item -ItemType Directory -Force -Path $artifacts, $fixture | Out-Null
$upgradeCode = '{1C80D5F7-A1EC-4BAE-A4A6-E010C5A3EE6B}'
if ($null -ne (Get-V7InstalledProduct $upgradeCode)) { throw 'EXE startup requires no existing HLS installation.' }
$nativeKey = 'HKCU:\Software\HLS Downloader\Native'
if (Test-Path -LiteralPath $nativeKey) { throw 'EXE startup requires no existing native installation settings.' }
$result = [ordered]@{ schema = 1; passed = $false; scenarios = @(); scope = 'Frozen EXE install, real Start Menu shortcut, visible responsive workbench; isolated hosted Windows only.' }
$ownedInstallDir = $null
$bundle = $null
$installed = $false

function Invoke-Bundle([string]$Action, [string]$Log) {
    $process = Start-Process -FilePath $bundle -ArgumentList @($Action, '/quiet', '/norestart', '/log', ('"' + $Log + '"')) -PassThru
    if (-not $process.WaitForExit(120000)) { throw 'Burn operation timed out; installation evidence was retained.' }
    $process.Refresh()
    if ($process.ExitCode -notin @(0, 3010, 1641)) { throw "Burn $Action failed with exit code $($process.ExitCode)." }
    return $process.ExitCode
}

function Stop-TestApplication {
    if (-not $ownedInstallDir) { return }
    @(Get-CimInstance Win32_Process | Where-Object {
        $_.ExecutablePath -and $_.ExecutablePath.StartsWith($ownedInstallDir.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)
    }) | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

try {
    $manifestPath = (Resolve-Path -LiteralPath $CandidateManifestPath).Path
    $manifest = [IO.File]::ReadAllText($manifestPath, [Text.Encoding]::UTF8) | ConvertFrom-Json
    $bundle = Join-Path (Split-Path $manifestPath -Parent) $manifest.artifacts.exe.path
    $msi = Join-Path (Split-Path $manifestPath -Parent) $manifest.artifacts.msi.path
    $exeHash = (Get-FileHash -LiteralPath $bundle -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($exeHash -ne $manifest.artifacts.exe.sha256) { throw 'Candidate EXE SHA256 mismatch.' }
    if ((Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLowerInvariant() -ne $manifest.artifacts.msi.sha256) { throw 'Candidate MSI SHA256 mismatch.' }
    $productCode = Get-V7MsiProperty $msi 'ProductCode'
    $result.source_commit = $manifest.source_commit
    $result.candidate_manifest_sha256 = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $result.candidate_exe_sha256 = $exeHash

    foreach ($scenario in @('fresh-default', 'legacy-custom-path')) {
        $scenarioRoot = Join-Path $artifacts $scenario
        New-Item -ItemType Directory -Force -Path $scenarioRoot | Out-Null
        $ownedInstallDir = if ($scenario -eq 'fresh-default') { Join-Path $env:LOCALAPPDATA 'HLSDownloader' } else { Join-Path $fixture ('custom install\HLS ' + [char]0x4e0b + [char]0x8f7d + [char]0x5668) }
        if (Test-Path -LiteralPath (Join-Path $ownedInstallDir 'HLSDownloader.exe')) { throw 'Refusing to replace an existing installation.' }
        if ($scenario -eq 'legacy-custom-path') {
            New-Item -Path $nativeKey -Force | Out-Null
            New-ItemProperty -LiteralPath $nativeKey -Name InstallDir -Value $ownedInstallDir -PropertyType String -Force | Out-Null
        }
        $data = Join-Path $scenarioRoot 'data'
        New-Item -ItemType Directory -Force -Path $data | Out-Null
        if ($scenario -eq 'legacy-custom-path') {
            # Reproduce the old candidate's confirmed schema-6 startup failure.
            @'
import pathlib, sqlite3, sys
with sqlite3.connect(pathlib.Path(sys.argv[2]) / 'data.db') as connection:
    connection.executescript(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
'@ | python - (Join-Path $repo 'native_shell\tests\fixtures\schema-v6.sql') $data
            if ($LASTEXITCODE -ne 0) { throw 'Legacy profile fixture failed.' }
        }
        $installed = $true
        $installExit = Invoke-Bundle '/install' (Join-Path $scenarioRoot 'install.log')
        $product = Get-V7InstalledProduct $upgradeCode
        if (-not $product -or $product.product_code -ne $productCode) { throw 'EXE did not register the exact candidate MSI.' }
        $actualDir = (Get-ItemProperty -LiteralPath $nativeKey -Name InstallDir).InstallDir
        if ($actualDir.TrimEnd('\') -ne $ownedInstallDir.TrimEnd('\')) { throw "Unexpected installation path: $actualDir" }
        foreach ($component in $manifest.components.PSObject.Properties) {
            if ((Get-FileHash -LiteralPath (Join-Path $ownedInstallDir $component.Name) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $component.Value) { throw "Installed component mismatch: $($component.Name)" }
        }
        $shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'HLS Downloader\HLS Downloader.lnk'
        $shell = New-Object -ComObject WScript.Shell
        try {
            $link = $shell.CreateShortcut($shortcut)
            $shortcutState = [ordered]@{ path = $shortcut; target = $link.TargetPath; working_directory = $link.WorkingDirectory; expected_directory = $ownedInstallDir }
            [IO.File]::WriteAllText((Join-Path $scenarioRoot 'shortcut.json'), ($shortcutState | ConvertTo-Json), $utf8)
            if (-not (Test-Path -LiteralPath $link.TargetPath -PathType Leaf)) { throw "Installed Start Menu target does not exist: $($link.TargetPath)" }
            if ((Get-FileHash -LiteralPath $link.TargetPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $manifest.components.'HLSDownloader.exe') { throw 'Installed Start Menu target is not the candidate launcher.' }
        } finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) | Out-Null }
        $probeReport = Join-Path $scenarioRoot 'window.json'
        $environment = @([Environment]::GetEnvironmentVariables().GetEnumerator() | Where-Object { $_.Key -notmatch '^HLS_' } | ForEach-Object { "$($_.Key)=$($_.Value)" }) + @(
            "HLS_V7_DATA_DIR=$data", "HLS_V7_DOWNLOAD_DIR=$(Join-Path $scenarioRoot 'downloads')",
            "HLS_V7_PIPE=\\.\pipe\HLSExeStartup-$([guid]::NewGuid().ToString('n'))",
            "HLS_V7_PRESENTER_TRACE_FILE=$(Join-Path $scenarioRoot 'presenter.log')"
        )
        # WMI supplies the interactive desktop, outside the hosted agent's
        # restrictive Job. The application still uses its production flags.
        $startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{
            ShowWindow = [uint16]0; CreateFlags = [uint32]0x01000000
            EnvironmentVariables = [string[]]$environment; WinstationDesktop = 'winsta0\default'
        }
        $command = 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $PSCommandPath + '" -ProbeInstallDir "' + $ownedInstallDir + '" -ProbeShortcut "' + $shortcut + '" -ProbeReport "' + $probeReport + '"'
        $created = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $command; CurrentDirectory = $repo; ProcessStartupInformation = $startup }
        if ($created.ReturnValue -ne 0) { throw "Interactive startup probe failed: $($created.ReturnValue)" }
        $deadline = (Get-Date).AddSeconds(65)
        do { Start-Sleep -Milliseconds 250 } while (-not (Test-Path -LiteralPath $probeReport) -and (Get-Date) -lt $deadline)
        if (-not (Test-Path -LiteralPath $probeReport)) { throw 'Startup probe produced no result.' }
        $probe = [IO.File]::ReadAllText($probeReport, [Text.Encoding]::UTF8) | ConvertFrom-Json
        $result.scenarios += [ordered]@{ name = $scenario; install_dir = $ownedInstallDir; install_exit = $installExit; components_match = $true; probe = $probe }
        if (-not $probe.passed) { throw $probe.error }
        Stop-TestApplication
        if ($scenario -eq 'legacy-custom-path') {
            $schema = & python -c "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); print(c.execute('pragma user_version').fetchone()[0]); c.close()" (Join-Path $data 'data.db')
            if ($LASTEXITCODE -ne 0 -or $schema -ne '7') { throw "Legacy database was not migrated to schema 7: $schema" }
            $result.scenarios[-1].migrated_schema = [int]$schema
        }
        $uninstallExit = Invoke-Bundle '/uninstall' (Join-Path $scenarioRoot 'uninstall.log')
        if ($null -ne (Get-V7InstalledProduct $upgradeCode)) { throw 'Bundle uninstall left the product registered.' }
        $installed = $false
        $result.scenarios[-1].uninstall_exit = $uninstallExit
    }
    $result.passed = $true
} catch { $result.error = $_.Exception.Message }
finally {
    Stop-TestApplication
    if ($installed -and $bundle) {
        try { [void](Invoke-Bundle '/uninstall' (Join-Path $artifacts 'cleanup.log')) }
        catch { $result.cleanup_error = $_.Exception.Message; $result.passed = $false }
    }
    [IO.File]::WriteAllText((Join-Path $artifacts 'result.json'), ($result | ConvertTo-Json -Depth 7), $utf8)
    $result | ConvertTo-Json -Depth 7
}
if (-not $result.passed) { exit 1 }
