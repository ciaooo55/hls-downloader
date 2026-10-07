[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Engine,
    [Parameter(Mandatory = $true)][string]$NativeHost,
    [Parameter(Mandatory = $true)][string]$WixRoot,
    [Parameter(Mandatory = $true)][string]$EvidenceRoot,
    [string]$PythonPath = 'python.exe',
    [ValidateSet('Lifecycle','PrivilegesGuard')][string]$Mode = 'Lifecycle'
)

$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$cache = [IO.Path]::GetFullPath((Join-Path $repo '.tool-cache\test-tmp'))
$root = Join-Path $cache ('msi-isolated-' + [guid]::NewGuid().ToString('N'))
$install = Join-Path $root 'installed'
$evidence = [IO.Path]::GetFullPath($EvidenceRoot)
if (-not $evidence.StartsWith((Join-Path $repo 'artifacts\v7-productization\'), [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Installer evidence must stay under artifacts/v7-productization.'
}
New-Item -ItemType Directory -Force -Path $root, $evidence | Out-Null
$enginePath = (Resolve-Path -LiteralPath $Engine).Path
$hostPath = (Resolve-Path -LiteralPath $NativeHost).Path
$upgradeCode = [guid]::NewGuid().ToString('B').ToUpperInvariant()
$oldCode = [guid]::NewGuid().ToString('B').ToUpperInvariant()
$newCode = [guid]::NewGuid().ToString('B').ToUpperInvariant()
$steps = New-Object Collections.ArrayList
$report = [ordered]@{
    scope = 'Isolated two-binary MSI lifecycle fixture using the production lifecycle patch; not a product package or release'
    install_dir = $install; upgrade_code = $upgradeCode; old_product = $oldCode; new_product = $newCode
    engine_sha256 = (Get-FileHash -LiteralPath $enginePath -Algorithm SHA256).Hash.ToLowerInvariant()
    native_host_sha256 = (Get-FileHash -LiteralPath $hostPath -Algorithm SHA256).Hash.ToLowerInvariant()
    passed = $false; steps = $steps
    mode = $Mode
}

function Assert-Step([string]$Name, [bool]$Passed, $Actual) {
    [void]$steps.Add([ordered]@{ name = $Name; passed = $Passed; actual = $Actual })
    if (-not $Passed) { throw "$Name failed: $Actual" }
}

function Edit-Msi([string]$Path, [string[]]$Queries, [bool]$ChangePackage = $false) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $db = $null; $summary = $null
    try {
        $db = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($Path, 1))
        foreach ($sql in $Queries) {
            $view = $db.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $db, @($sql))
            try { $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null }
            finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) | Out-Null }
        }
        if ($ChangePackage) {
            $summary = $db.GetType().InvokeMember('SummaryInformation', 'GetProperty', $null, $db, 1)
            $summary.GetType().InvokeMember('Property', 'SetProperty', $null, $summary, @(9, [guid]::NewGuid().ToString('B').ToUpperInvariant())) | Out-Null
            $summary.GetType().InvokeMember('Persist', 'InvokeMethod', $null, $summary, $null) | Out-Null
        }
        $db.GetType().InvokeMember('Commit', 'InvokeMethod', $null, $db, $null) | Out-Null
    } finally {
        foreach ($item in @($summary, $db, $installer)) {
            if ($null -ne $item) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($item) | Out-Null }
        }
    }
}

function Product-State([string]$Code) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    try { return $installer.GetType().InvokeMember('ProductState', 'GetProperty', $null, $installer, @($Code)) }
    finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null }
}

function Invoke-Msi([string[]]$Arguments, [string]$Log) {
    $process = Start-Process -FilePath msiexec.exe -ArgumentList ($Arguments + @('/norestart', '/l*v', ('"' + (Join-Path $evidence $Log) + '"'))) -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(60000)) { throw "MSI transaction did not finish within 60 seconds: $Log" }
    return $process.ExitCode
}

function Free-Port {
    $listener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0)
    try { $listener.Start(); return ([Net.IPEndPoint]$listener.LocalEndpoint).Port }
    finally { $listener.Stop() }
}

function Checkpoint([string]$Mode, [string]$Name) {
    & $python (Join-Path $PSScriptRoot 'msi_checkpoint_fixture.py') --mode $Mode --engine (Join-Path $install 'app\resources\HLSDownloaderEngine.exe') --root (Join-Path $root 'checkpoint') --state (Join-Path $root 'checkpoint\state.json') --core-port $corePort --origin-port $originPort --report (Join-Path $evidence ($Name + '.json')) *> (Join-Path $evidence ($Name + '.log'))
    Assert-Step $Name ($LASTEXITCODE -eq 0) $LASTEXITCODE
}

function Build-Fixture([string]$Label, [string]$Code) {
    $marker = Join-Path $root ($Label + '.txt')
    [IO.File]::WriteAllText($marker, $Label, (New-Object Text.UTF8Encoding($false)))
    $wxs = Join-Path $root ($Label + '.wxs')
    $object = Join-Path $root ($Label + '.wixobj')
    $msi = Join-Path $evidence ($Label + '-fixture.msi')
    $xmlEngine = [Security.SecurityElement]::Escape($enginePath)
    $xmlHost = [Security.SecurityElement]::Escape($hostPath)
    $xmlMarker = [Security.SecurityElement]::Escape($marker)
    $xml = @"
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi" xmlns:util="http://schemas.microsoft.com/wix/UtilExtension">
 <Product Id="$Code" Name="HLS Downloader Isolated Installer Test" Language="1033" Version="7.0.2" Manufacturer="HLS Downloader Tests" UpgradeCode="$upgradeCode">
  <Package InstallerVersion="500" Compressed="yes" InstallScope="perUser" InstallPrivileges="limited" Platform="x64"/>
  <MediaTemplate EmbedCab="yes"/>
  <Upgrade Id="$upgradeCode"><UpgradeVersion Maximum="7.0.2" IncludeMaximum="yes" Property="JP_UPGRADABLE_FOUND"/></Upgrade>
  <Property Id="SAVED_INSTALLDIR"><RegistrySearch Id="FindSavedDir" Root="HKCU" Key="Software\HLSDownloaderIsolated\$upgradeCode" Name="InstallDir" Type="raw"/></Property>
  <Directory Id="TARGETDIR" Name="SourceDir"><Directory Id="LocalAppDataFolder"><Directory Id="INSTALLDIR" Name="HLSDownloaderIsolated"><Directory Id="APPDIR" Name="app"><Directory Id="RESOURCES" Name="resources">
   <Component Id="EngineComponent" Guid="*" Win64="yes"><File Id="EngineFile" Name="HLSDownloaderEngine.exe" Source="$xmlEngine" KeyPath="yes"/></Component>
   <Component Id="HostComponent" Guid="*" Win64="yes"><File Id="HostFile" Name="HLSDownloaderNativeHost.exe" Source="$xmlHost" KeyPath="yes"/></Component>
   <Component Id="MarkerComponent" Guid="*" Win64="yes"><File Id="MarkerFile" Name="installed-version.txt" Source="$xmlMarker" KeyPath="yes"/></Component>
  </Directory></Directory>
   <Component Id="DirectoryComponent" Guid="*" Win64="yes"><RegistryValue Root="HKCU" Key="Software\HLSDownloaderIsolated\$upgradeCode" Name="InstallDir" Value="[INSTALLDIR]" Type="string" KeyPath="yes"/><util:RemoveFolderEx On="uninstall" Property="SAVED_INSTALLDIR"/></Component>
  </Directory></Directory></Directory>
  <Feature Id="Main" Title="Fixture" Level="1"><ComponentRef Id="EngineComponent"/><ComponentRef Id="HostComponent"/><ComponentRef Id="MarkerComponent"/><ComponentRef Id="DirectoryComponent"/></Feature>
  <InstallExecuteSequence><RemoveExistingProducts Before="InstallInitialize"/></InstallExecuteSequence>
 </Product>
</Wix>
"@
    [IO.File]::WriteAllText($wxs, $xml, (New-Object Text.UTF8Encoding($false)))
    & (Join-Path $WixRoot 'candle.exe') $wxs -out $object -nologo -ext (Join-Path $WixRoot 'WixUtilExtension.dll') *> (Join-Path $evidence ($Label + '-candle.log'))
    if ($LASTEXITCODE -ne 0) { throw "WiX candle failed: $Label" }
    & (Join-Path $WixRoot 'light.exe') $object -out $msi -nologo -sval -ext (Join-Path $WixRoot 'WixUtilExtension.dll') *> (Join-Path $evidence ($Label + '-light.log'))
    if ($LASTEXITCODE -ne 0) { throw "WiX light failed: $Label" }
    & (Join-Path $PSScriptRoot 'set-v7-msi-rollback-order.ps1') -MsiPath $msi -ProductCode $Code *> (Join-Path $evidence ($Label + '-production-patch.json'))
    # Fixture components/registry never share ownership with the user's installed product.
    $queries = @()
    foreach ($name in @('Chrome','Edge','Brave','Chromium','Vivaldi','Opera','Firefox')) {
        $guid = [guid]::NewGuid().ToString('B').ToUpperInvariant()
        $queries += "UPDATE ``Registry`` SET ``Key``='Software\HLSDownloaderIsolated\$upgradeCode\NativeHost' WHERE ``Registry``='V7NativeHost${name}Registry'"
        $queries += "UPDATE ``Component`` SET ``ComponentId``='$guid' WHERE ``Component``='V7NativeHost${name}Component'"
        $queries += "UPDATE ``Registry`` SET ``Name``='$name' WHERE ``Registry``='V7NativeHost${name}Registry'"
    }
    Edit-Msi $msi $queries $true
    return $msi
}

try {
    $administrator = (New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    Assert-Step 'matching-privilege-context' (($Mode -eq 'Lifecycle' -and $administrator) -or ($Mode -eq 'PrivilegesGuard' -and -not $administrator)) $administrator
    $old = Build-Fixture 'baseline' $oldCode
    $new = Build-Fixture 'updated' $newCode
    $code = Invoke-Msi @('/i', ('"' + $old + '"'), '/qn', ('INSTALLDIR="' + $install + '"')) 'baseline-install.log'
    Assert-Step 'baseline-install' ($code -in @(0,3010)) $code
    Assert-Step 'baseline-registered' ((Product-State $oldCode) -eq 5) (Product-State $oldCode)
    $oldHash = (Get-FileHash -LiteralPath (Join-Path $install 'app\resources\HLSDownloaderEngine.exe')).Hash
    if ($Mode -eq 'PrivilegesGuard') {
        $code = Invoke-Msi @('/i', ('"' + $new + '"'), '/qn', ('INSTALLDIR="' + $install + '"')) 'guard-rejection.log'
        Assert-Step 'unprivileged-upgrade-refused' ($code -eq 1603) $code
        Assert-Step 'guard-message' ((Get-Content (Join-Path $evidence 'guard-rejection.log') -Raw).Contains('Please run this upgrade as administrator')) 'upgrade privilege error'
        Assert-Step 'guard-keeps-old-registration' ((Product-State $oldCode) -eq 5) (Product-State $oldCode)
        Assert-Step 'guard-keeps-old-files' ((Get-FileHash -LiteralPath (Join-Path $install 'app\resources\HLSDownloaderEngine.exe')).Hash -eq $oldHash -and (Get-Content (Join-Path $install 'app\resources\installed-version.txt') -Raw) -eq 'baseline') 'baseline engine/marker'
        Assert-Step 'guard-new-not-installed' ((Product-State $newCode) -ne 5) (Product-State $newCode)
        $report.passed = $true
    } else {
    $python = (Get-Command $PythonPath -ErrorAction Stop).Source
    $corePort = Free-Port; $originPort = Free-Port
    Checkpoint 'create-pause' 'checkpoint-before-upgrade'
    $forced = Join-Path $evidence 'forced-rollback-fixture.msi'
    Copy-Item -LiteralPath $new -Destination $forced
    Edit-Msi $forced @(
        'INSERT INTO `CustomAction` (`Action`,`Type`,`Source`,`Target`) VALUES (''V7ForcedRollback'',1058,''TARGETDIR'',''"[SystemFolder]cmd.exe" /d /c exit /b 1'')',
        'INSERT INTO `InstallExecuteSequence` (`Action`,`Condition`,`Sequence`) VALUES (''V7ForcedRollback'',''NOT Installed'',6599)'
    ) $true
    $code = Invoke-Msi @('/i', ('"' + $forced + '"'), '/qn', ('INSTALLDIR="' + $install + '"')) 'forced-rollback.log'
    Assert-Step 'forced-failure' ($code -eq 1603) $code
    Assert-Step 'rollback-old-registration' ((Product-State $oldCode) -eq 5) (Product-State $oldCode)
    Assert-Step 'rollback-new-not-installed' ((Product-State $newCode) -ne 5) (Product-State $newCode)
    Assert-Step 'rollback-old-files' ((Get-FileHash -LiteralPath (Join-Path $install 'app\resources\HLSDownloaderEngine.exe')).Hash -eq $oldHash -and (Get-Content (Join-Path $install 'app\resources\installed-version.txt') -Raw) -eq 'baseline') 'baseline engine/marker'
    Checkpoint 'verify-paused' 'checkpoint-after-rollback'
    $code = Invoke-Msi @('/i', ('"' + $new + '"'), '/qn', ('INSTALLDIR="' + $install + '"')) 'upgrade.log'
    Assert-Step 'upgrade-install' ($code -in @(0,3010)) $code
    Assert-Step 'upgrade-old-unregistered' ((Product-State $oldCode) -ne 5) (Product-State $oldCode)
    Assert-Step 'upgrade-new-registered' ((Product-State $newCode) -eq 5) (Product-State $newCode)
    Assert-Step 'upgrade-files' ((Get-Content (Join-Path $install 'app\resources\installed-version.txt') -Raw) -eq 'updated') 'updated marker'
    Checkpoint 'verify-resume' 'checkpoint-after-upgrade'
    $hostReport = Join-Path $evidence 'installed-native-host.json'
    try {
        # 首响应性能失败仍写出功能证据；PS 5.1 不能因其 stderr 提前跳过读取。
        $ErrorActionPreference = 'Continue'
        & $python (Join-Path $PSScriptRoot 'smoke_v7_native_host.py') --host (Join-Path $install 'app\resources\HLSDownloaderNativeHost.exe') --engine (Join-Path $install 'app\resources\HLSDownloaderEngine.exe') --stage-root $root --profile-startup --report $hostReport *> (Join-Path $evidence 'installed-native-host.log')
    } finally { $ErrorActionPreference = 'Stop' }
    $hostEvidence = Get-Content -LiteralPath $hostReport -Raw -Encoding UTF8 | ConvertFrom-Json
    Assert-Step 'installed-native-host-functional' ($hostEvidence.resident_engine_count -eq 1 -and $hostEvidence.two_response_total_ms -gt 0) $hostEvidence
    $code = Invoke-Msi @('/x', $newCode, '/qn') 'uninstall.log'
    Assert-Step 'uninstall' ($code -in @(0,3010)) $code
    Assert-Step 'uninstall-unregistered' ((Product-State $newCode) -ne 5) (Product-State $newCode)
    Assert-Step 'uninstall-files-removed' (-not (Test-Path -LiteralPath (Join-Path $install 'app\resources\HLSDownloaderEngine.exe'))) $install
    $report.passed = $true
    }
} catch {
    $report.error = $_.Exception.Message
} finally {
    foreach ($code in @($oldCode,$newCode)) {
        if ((Product-State $code) -ge 1) {
            $exit = Invoke-Msi @('/x',$code,'/qn') ('cleanup-' + $code.Trim('{}') + '.log')
            Assert-Step 'fixture-product-cleaned' ($exit -in @(0,3010,1605) -and (Product-State $code) -lt 1) $exit
        }
    }
    [IO.File]::WriteAllText((Join-Path $evidence 'result.json'), ($report | ConvertTo-Json -Depth 12), (New-Object Text.UTF8Encoding($false)))
}
if (-not $report.passed) { throw $report.error }
