[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Upgrade', 'FailureRollback')]
    [string]$Scenario,

    [Parameter(Mandatory = $true)]
    [string]$CandidateManifestPath,

    [string]$OldMsiPath = '',
    [string]$OldMsiUrl = 'https://github.com/ciaooo55/hls-downloader/releases/download/v7.0.0/HLSDownloader-7.0.0-Windows-x64.msi',
    [string]$InstallDir = 'E:\h',
    [string]$CheckpointPath = '',
    [string]$ReportPath = ''
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$artifacts = Join-Path $repo 'artifacts\v7-msi-lifecycle'
$utf8NoBom = New-Object Text.UTF8Encoding($false)
$started = (Get-Date).ToUniversalTime().ToString('o')
$steps = New-Object Collections.ArrayList
$installedProductCode = $null
$result = $null
$previousDataDir = $env:HLS_V7_DATA_DIR
$previousDownloadDir = $env:HLS_V7_DOWNLOAD_DIR

$expectedInstallDir = [IO.Path]::GetFullPath('E:\h').TrimEnd('\', '/')
$InstallDir = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\', '/')
if (-not [String]::Equals($InstallDir, $expectedInstallDir, [StringComparison]::OrdinalIgnoreCase)) {
    throw "MSI lifecycle install directory must be exactly ${expectedInstallDir}: $InstallDir"
}

function Add-Step([string]$Name, [bool]$Passed, $Actual) {
    [void]$steps.Add([ordered]@{ name = $Name; passed = $Passed; actual = $Actual })
    if (-not $Passed) { throw "$Name failed: $Actual" }
}

function Resolve-FullPath([string]$Path, [string]$Base) {
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $Base $Path))
}

function Get-MsiProperty([string]$Path, [string]$Property) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $null
    $view = $null
    $record = $null
    try {
        $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($Path, 0))
        $sql = "SELECT ``Value`` FROM ``Property`` WHERE ``Property``='$($Property.Replace("'", "''"))'"
        $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @($sql))
        $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
        $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
        if ($null -eq $record) { return $null }
        return [string]$record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, 1)
    } finally {
        foreach ($item in @($record, $view, $database, $installer)) {
            if ($null -ne $item) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($item) | Out-Null }
        }
    }
}

function Invoke-Msi([string[]]$Arguments, [string]$LogPath) {
    $allArguments = @($Arguments) + @('/norestart', 'REBOOT=ReallySuppress', '/l*v', $LogPath)
    $process = Start-Process -FilePath 'msiexec.exe' -ArgumentList $allArguments -Wait -PassThru
    return [int]$process.ExitCode
}

function Get-TreeDigest([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    if (Test-Path -LiteralPath $Path -PathType Leaf) {
        return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    $lines = @(Get-ChildItem -LiteralPath $Path -File -Recurse | Sort-Object FullName | ForEach-Object {
        $relative = $_.FullName.Substring($Path.TrimEnd('\').Length).TrimStart('\')
        "$relative`t$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant())"
    })
    $sha = [Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(($lines -join "`n"))))).Replace('-', '').ToLowerInvariant() }
    finally { $sha.Dispose() }
}

function Get-InstalledProduct([string]$UpgradeCode) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    try {
        foreach ($productCode in @($installer.RelatedProducts($UpgradeCode))) {
            return [ordered]@{
                product_code = [string]$productCode
                version = [string]$installer.ProductInfo($productCode, 'VersionString')
            }
        }
        return $null
    } finally {
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null
    }
}

function Get-RegistrationState {
    $parents = @(
        'HKCU:\Software\Google\Chrome\NativeMessagingHosts',
        'HKCU:\Software\Microsoft\Edge\NativeMessagingHosts',
        'HKCU:\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts',
        'HKCU:\Software\Chromium\NativeMessagingHosts',
        'HKCU:\Software\Vivaldi\NativeMessagingHosts',
        'HKCU:\Software\Opera Software\NativeMessagingHosts',
        'HKCU:\Software\Mozilla\NativeMessagingHosts'
    )
    return @($parents | ForEach-Object {
        if (Test-Path -LiteralPath $_) {
            Get-ChildItem -LiteralPath $_ | Where-Object { $_.PSChildName -match 'hls.?downloader' } | Select-Object -ExpandProperty Name
        }
    })
}

function Get-ShortcutState {
    $roots = @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('StartMenu'))
    return @($roots | ForEach-Object {
        if (Test-Path -LiteralPath $_) { Get-ChildItem -LiteralPath $_ -Filter '*HLS*Downloader*.lnk' -File -Recurse -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName }
    })
}

function Start-InstalledApplication([string]$Root) {
    $exe = Join-Path $Root 'HLSDownloader.exe'
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Installed application is missing: $exe" }
    $launcher = Start-Process -FilePath $exe -WorkingDirectory $Root -PassThru
    $deadline = (Get-Date).AddSeconds(60)
    do {
        Start-Sleep -Milliseconds 250
        $engine = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
            $_.Name -eq 'HLSDownloaderEngine.exe' -and $_.ExecutablePath -and $_.ExecutablePath.StartsWith($Root, [StringComparison]::OrdinalIgnoreCase)
        }) | Select-Object -First 1
        $presenter = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
            $_.Name -eq 'HLSDownloaderPresenter.exe' -and $_.ExecutablePath -and $_.ExecutablePath.StartsWith($Root, [StringComparison]::OrdinalIgnoreCase)
        }) | Select-Object -First 1
    } while ((-not $engine -or -not $presenter) -and (Get-Date) -lt $deadline)
    if (-not $engine -or -not $presenter) { throw 'Installed application did not start its Engine and Presenter processes.' }
    return [ordered]@{ launcher_pid = [int]$launcher.Id; engine_pid = [int]$engine.ProcessId; presenter_pid = [int]$presenter.ProcessId }
}

function Stop-InstalledProcesses([string]$Root) {
    @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        $_.ExecutablePath -and $_.ExecutablePath.StartsWith($Root, [StringComparison]::OrdinalIgnoreCase)
    }) | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

function Get-FreeTcpPort {
    $listener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0)
    try { $listener.Start(); return ([Net.IPEndPoint]$listener.LocalEndpoint).Port }
    finally { $listener.Stop() }
}

function Invoke-CheckpointFixture([string]$Mode, [string]$Engine, [string]$Report) {
    $arguments = @(
        (Join-Path $repo 'scripts\msi_checkpoint_fixture.py'), '--mode', $Mode,
        '--engine', $Engine, '--root', $fixtureRoot, '--state', $fixtureState,
        '--report', $Report, '--core-port', [string]$corePort, '--origin-port', [string]$originPort
    )
    & $python @arguments | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "MSI checkpoint fixture $Mode failed with exit code $LASTEXITCODE." }
    return [IO.File]::ReadAllText($Report, [Text.Encoding]::UTF8) | ConvertFrom-Json
}

function Add-Type19Failure([string]$Path) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $null
    try {
        $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($Path, 1))
        foreach ($sql in @(
            "DELETE FROM ``InstallExecuteSequence`` WHERE ``Action``='V7ForcedRollback'",
            "DELETE FROM ``CustomAction`` WHERE ``Action``='V7ForcedRollback'",
            "INSERT INTO ``CustomAction`` (``Action``,``Type``,``Source``,``Target``) VALUES ('V7ForcedRollback',19,'','Injected lifecycle rollback failure')",
            "INSERT INTO ``InstallExecuteSequence`` (``Action``,``Condition``,``Sequence``) VALUES ('V7ForcedRollback','NOT Installed',6590)"
        )) {
            $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @($sql))
            try { $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null }
            finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) | Out-Null }
        }
        $database.GetType().InvokeMember('Commit', 'InvokeMethod', $null, $database, $null) | Out-Null
    } finally {
        if ($null -ne $database) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) | Out-Null }
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null
    }
}

try {
    New-Item -ItemType Directory -Force -Path $artifacts | Out-Null
    $pythonCommand = Get-Command $(if ($env:HLS_V7_PYTHON) { $env:HLS_V7_PYTHON } else { 'python.exe' }) -ErrorAction Stop
    $python = $pythonCommand.Source
    $fixtureRoot = Join-Path $artifacts "$($Scenario.ToLowerInvariant())-checkpoint"
    $fixtureState = Join-Path $fixtureRoot 'task-state.json'
    New-Item -ItemType Directory -Force -Path $fixtureRoot | Out-Null
    $env:HLS_V7_DATA_DIR = Join-Path $fixtureRoot 'data'
    $env:HLS_V7_DOWNLOAD_DIR = Join-Path $fixtureRoot 'downloads'
    $corePort = Get-FreeTcpPort
    $originPort = Get-FreeTcpPort
    $manifestPath = (Resolve-Path -LiteralPath $CandidateManifestPath).Path
    $manifest = [IO.File]::ReadAllText($manifestPath, [Text.Encoding]::UTF8) | ConvertFrom-Json
    $candidate = Resolve-FullPath ([string]$manifest.artifacts.msi.path) (Split-Path $manifestPath -Parent)
    Add-Step 'candidate-msi-exists' (Test-Path -LiteralPath $candidate -PathType Leaf) $candidate
    $candidateHash = (Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash.ToLowerInvariant()
    Add-Step 'candidate-manifest-sha256' ($candidateHash -eq ([string]$manifest.artifacts.msi.sha256).ToLowerInvariant()) $candidateHash

    $old = if ($OldMsiPath) { (Resolve-Path -LiteralPath $OldMsiPath).Path } else { Join-Path $artifacts 'HLSDownloader-7.0.0-Windows-x64.msi' }
    if (-not (Test-Path -LiteralPath $old -PathType Leaf)) { Invoke-WebRequest -Uri $OldMsiUrl -OutFile $old -UseBasicParsing }
    Add-Step 'old-msi-version' ((Get-MsiProperty $old 'ProductVersion') -eq '7.0.0') (Get-MsiProperty $old 'ProductVersion')

    $candidateVersion = Get-MsiProperty $candidate 'ProductVersion'
    $upgradeCode = Get-MsiProperty $candidate 'UpgradeCode'
    $candidateProductCode = Get-MsiProperty $candidate 'ProductCode'
    $oldProductCode = Get-MsiProperty $old 'ProductCode'
    Add-Step 'candidate-manifest-version' (([string]$manifest.product_version) -eq '7.0.1') ([string]$manifest.product_version)
    Add-Step 'candidate-msi-version' ($candidateVersion -eq '7.0.1') $candidateVersion
    Add-Step 'upgrade-code-match' ($upgradeCode -eq '{1C80D5F7-A1EC-4BAE-A4A6-E010C5A3EE6B}' -and (Get-MsiProperty $old 'UpgradeCode') -eq $upgradeCode) $upgradeCode
    Add-Step 'candidate-product-code-is-new' ($candidateProductCode -ne $oldProductCode) $candidateProductCode

    $installLog = Join-Path $artifacts "$($Scenario.ToLowerInvariant())-install-old.log"
    $exit = Invoke-Msi @('/i', $old, '/qn', "INSTALLDIR=$InstallDir") $installLog
    Add-Step 'install-old-exit' ($exit -in @(0, 3010, 1641)) $exit
    $oldProduct = Get-InstalledProduct $upgradeCode
    Add-Step 'old-product-registered' ($null -ne $oldProduct -and $oldProduct.version -eq '7.0.0') $oldProduct
    $installedProductCode = $oldProduct.product_code
    $enginePath = Join-Path $InstallDir 'app\resources\HLSDownloaderEngine.exe'
    $engineBefore = (Get-FileHash -LiteralPath $enginePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $registrationBefore = @(Get-RegistrationState)
    $checkpointBefore = Invoke-CheckpointFixture 'create-pause' $enginePath (Join-Path $fixtureRoot 'before.json')
    Add-Step 'real-task-checkpoint-created' (
        $checkpointBefore.status -eq 'paused' -and
        [int64]$checkpointBefore.downloaded_bytes -gt 0 -and
        [int]$checkpointBefore.active_workers -eq 0
    ) $checkpointBefore

    if ($Scenario -eq 'Upgrade') {
        $beforeProcess = Start-InstalledApplication $InstallDir
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repo 'scripts\shutdown-running.ps1') -InstallDir $InstallDir
        Add-Step 'checkpointed-process-shutdown' ($LASTEXITCODE -eq 0) $LASTEXITCODE
        Add-Step 'old-engine-process-ended' ($null -eq (Get-Process -Id $beforeProcess.engine_pid -ErrorAction SilentlyContinue)) $beforeProcess.engine_pid
        Add-Step 'old-presenter-process-ended' ($null -eq (Get-Process -Id $beforeProcess.presenter_pid -ErrorAction SilentlyContinue)) $beforeProcess.presenter_pid
        $exit = Invoke-Msi @('/i', $candidate, '/qn', "INSTALLDIR=$InstallDir") (Join-Path $artifacts 'upgrade-candidate.log')
        Add-Step 'upgrade-exit' ($exit -in @(0, 3010, 1641)) $exit
        $afterProduct = Get-InstalledProduct $upgradeCode
        Add-Step 'product-upgraded' ($afterProduct.version -eq '7.0.1' -and $afterProduct.product_code -ne $oldProduct.product_code) $afterProduct
        Add-Step 'old-application-process-ended' ($null -eq (Get-Process -Id $beforeProcess.launcher_pid -ErrorAction SilentlyContinue)) $beforeProcess.launcher_pid
        $afterProcess = Start-InstalledApplication $InstallDir
        Add-Step 'application-process-restarted' ($afterProcess.engine_pid -ne $beforeProcess.engine_pid -and $afterProcess.presenter_pid -ne $beforeProcess.presenter_pid) $afterProcess
        Stop-InstalledProcesses $InstallDir
        $checkpointAfter = Invoke-CheckpointFixture 'verify-resume' $enginePath (Join-Path $fixtureRoot 'after-upgrade.json')
        Add-Step 'task-restored-and-resumed' (
            $checkpointAfter.task_id -eq $checkpointBefore.task_id -and
            [int64]$checkpointAfter.downloaded_bytes -ge [int64]$checkpointBefore.downloaded_bytes -and
            $checkpointAfter.status -eq 'completed'
        ) $checkpointAfter
        Add-Step 'registration-present' (@(Get-RegistrationState).Count -gt 0) (Get-RegistrationState)
        Add-Step 'shortcuts-present' (@(Get-ShortcutState).Count -gt 0) (Get-ShortcutState)
        $installedProductCode = $afterProduct.product_code
    } else {
        $oldProcess = Start-InstalledApplication $InstallDir
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repo 'scripts\shutdown-running.ps1') -InstallDir $InstallDir
        Add-Step 'old-application-checkpointed' ($LASTEXITCODE -eq 0) $LASTEXITCODE
        $rollbackMsi = Join-Path $artifacts 'HLSDownloader-7.0.1-forced-rollback.msi'
        Copy-Item -LiteralPath $candidate -Destination $rollbackMsi -Force
        Add-Type19Failure $rollbackMsi
        Add-Step 'candidate-original-unchanged' (((Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash.ToLowerInvariant()) -eq $candidateHash) $candidateHash
        $exit = Invoke-Msi @('/i', $rollbackMsi, '/qn', "INSTALLDIR=$InstallDir") (Join-Path $artifacts 'failure-rollback.log')
        Add-Step 'forced-failure-exit' ($exit -eq 1603) $exit
        $afterProduct = Get-InstalledProduct $upgradeCode
        Add-Step 'old-product-code-preserved' ($afterProduct.product_code -eq $oldProduct.product_code) $afterProduct.product_code
        Add-Step 'old-engine-preserved' (((Get-FileHash -LiteralPath $enginePath -Algorithm SHA256).Hash.ToLowerInvariant()) -eq $engineBefore) $engineBefore
        $databaseAfterFailure = (Get-FileHash -LiteralPath ([string]$checkpointBefore.data_db) -Algorithm SHA256).Hash.ToLowerInvariant()
        Add-Step 'database-bytes-preserved' ($databaseAfterFailure -eq $checkpointBefore.data_db_sha256) $databaseAfterFailure
        $checkpointAfter = Invoke-CheckpointFixture 'verify-paused' $enginePath (Join-Path $fixtureRoot 'after-rollback.json')
        Add-Step 'task-and-data-preserved' (
            $checkpointAfter.task_id -eq $checkpointBefore.task_id -and
            $checkpointAfter.status -eq 'paused' -and
            [int64]$checkpointAfter.downloaded_bytes -ge [int64]$checkpointBefore.downloaded_bytes
        ) $checkpointAfter
        $registrationAfter = @(Get-RegistrationState)
        Add-Step 'registration-preserved' ((@($registrationAfter) -join "`n") -eq (@($registrationBefore) -join "`n")) $registrationAfter
        Add-Step 'old-application-launches' ($null -ne (Start-InstalledApplication $InstallDir)) $true
    }

    Stop-InstalledProcesses $InstallDir
    $uninstallExit = Invoke-Msi @('/x', $installedProductCode, '/qn') (Join-Path $artifacts "$($Scenario.ToLowerInvariant())-uninstall.log")
    Add-Step 'uninstall-exit' ($uninstallExit -in @(0, 3010, 1641)) $uninstallExit
    Add-Step 'product-unregistered' ($null -eq (Get-InstalledProduct $upgradeCode)) $installedProductCode
    $installedProductCode = $null
    $result = [ordered]@{ schema = 1; scenario = $Scenario; status = 'passed'; restart_scope = 'application-process-only'; system_reboot = $false; install_dir = $InstallDir; candidate_manifest = $manifestPath; candidate_msi = $candidate; old_msi = $old; started_at = $started; finished_at = (Get-Date).ToUniversalTime().ToString('o'); steps = $steps }
} catch {
    $result = [ordered]@{ schema = 1; scenario = $Scenario; status = 'failed'; restart_scope = 'application-process-only'; system_reboot = $false; install_dir = $InstallDir; started_at = $started; finished_at = (Get-Date).ToUniversalTime().ToString('o'); error = $_.Exception.Message; steps = $steps }
} finally {
    Stop-InstalledProcesses $InstallDir
    if ($installedProductCode) {
        try { [void](Invoke-Msi @('/x', $installedProductCode, '/qn') (Join-Path $artifacts "$($Scenario.ToLowerInvariant())-cleanup.log")) } catch { }
    }
    if (-not $ReportPath) { $ReportPath = Join-Path $artifacts "$($Scenario.ToLowerInvariant()).json" }
    $report = Resolve-FullPath $ReportPath $repo
    New-Item -ItemType Directory -Force -Path (Split-Path $report -Parent) | Out-Null
    [IO.File]::WriteAllText($report, ($result | ConvertTo-Json -Depth 8), $utf8NoBom)
    $result | ConvertTo-Json -Depth 8
    $env:HLS_V7_DATA_DIR = $previousDataDir
    $env:HLS_V7_DOWNLOAD_DIR = $previousDownloadDir
}

if ($result.status -ne 'passed') { exit 1 }
