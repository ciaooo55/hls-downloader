$ErrorActionPreference = 'Stop'
$qa = 'C:\HLSInstallerQA'
$work = 'C:\QAWork'
$product = '{7C42C864-FE5D-3D04-A5FC-0106F0441BEC}'
$installed = $false
$report = [ordered]@{ passed=$false; checks=@() }
try {
    New-Item -ItemType Directory -Path $work -Force | Out-Null
    $msi = Join-Path $work 'fixed.msi'
    Copy-Item -LiteralPath "$qa\fixed.msi" -Destination $msi
    $report.msi_sha256 = (Get-FileHash -LiteralPath $msi -Algorithm SHA256).Hash.ToLowerInvariant()
    'wizard' | Set-Content "$qa\phase.txt"
    $wizard = Start-Process msiexec.exe -ArgumentList "/i `"$msi`" /norestart /l*v `"$work\wizard.log`"" -PassThru
    $null = $wizard.Handle
    if (-not $wizard.WaitForExit(600000)) { throw 'Wizard timed out' }
    $report.wizard_exit = $wizard.ExitCode
    if ($wizard.ExitCode -ne 1602) { throw "Expected cancellation after checking wizard pages, got $($wizard.ExitCode)" }
    $report.checks += 'wizard cancellation returned 1602'
    'install' | Set-Content "$qa\phase.txt"
    $install = Join-Path $work 'Custom Install Path\HLSDownloader'
    $p = Start-Process msiexec.exe -ArgumentList "/i `"$msi`" /qn /norestart INSTALLDIR=`"$install`" /l*v `"$work\install.log`"" -PassThru -Wait
    $report.install_exit = $p.ExitCode
    if ($p.ExitCode -notin @(0,3010)) { throw "Silent install failed: $($p.ExitCode)" }
    $installed = $true
    $expected = Get-Content -LiteralPath 'C:\HLSSource\docs\validation\delivery-20261019\expected.json' -Raw | ConvertFrom-Json
    $components = [ordered]@{}
    foreach ($item in $expected.components_sha256.PSObject.Properties) {
        $actual = (Get-FileHash -LiteralPath (Join-Path $install $item.Name) -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $item.Value) { throw "Component changed: $($item.Name)" }
        $components[$item.Name] = $actual
    }
    $report.components_sha256 = $components
    $report.install_directory = $install
    $report.checks += 'custom-path install and all 12 packaged component hashes match'
    'uninstall' | Set-Content "$qa\phase.txt"
    $p = Start-Process msiexec.exe -ArgumentList "/x $product /qn /norestart /l*v `"$work\uninstall.log`"" -PassThru -Wait
    $report.uninstall_exit = $p.ExitCode
    if ($p.ExitCode -notin @(0,3010)) { throw "Uninstall failed: $($p.ExitCode)" }
    $installed = $false
    if (Test-Path -LiteralPath (Join-Path $install 'HLSDownloader.exe')) { throw 'Installed executable remains' }
    $installer = New-Object -ComObject WindowsInstaller.Installer
    try {
        $state = $installer.GetType().InvokeMember('ProductState','GetProperty',$null,$installer,@($product))
        if ($state -ne -1) { throw "Product still registered: $state" }
    } finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null }
    $report.checks += 'uninstall removed product registration and installed executable'
    $report.passed = $true
} catch {
    $report.error = $_ | Out-String
} finally {
    if ($installed) { Start-Process msiexec.exe -ArgumentList "/x $product /qn /norestart" -Wait }
    Get-ChildItem -LiteralPath $work -Filter '*.log' -File | Copy-Item -Destination $qa
    $report | ConvertTo-Json -Depth 7 | Set-Content "$qa\result.json" -Encoding utf8
    'done' | Set-Content "$qa\phase.txt"
}
