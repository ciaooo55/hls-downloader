$ErrorActionPreference = 'Stop'
$result = [ordered]@{ passed = $false; environment = 'Windows Sandbox'; host_installation = $false }
$install = 'C:\QAWork\Upgrade HLSDownloader'
try {
    $expected = Get-Content C:\QAInput\root-cause-package-expected.json -Raw | ConvertFrom-Json
    $old = Start-Process msiexec.exe -ArgumentList '/i "C:\QAInput\candidate.msi" /qn /norestart INSTALLDIR="C:\QAWork\Upgrade HLSDownloader" /l*v "C:\QAInput\root-cause-old-install.log"' -Wait -PassThru -WindowStyle Hidden
    if ($old.ExitCode -ne 0) { throw "Old install failed: $($old.ExitCode)" }
    & C:\QAPython\python.exe C:\QAInput\scripts\msi_checkpoint_fixture.py --mode create-pause --engine "$install\app\resources\HLSDownloaderEngine.exe" --root C:\QAWork\UpgradeState --state C:\QAInput\root-cause-checkpoint.json --report C:\QAInput\root-cause-before-upgrade.json --core-port 39187 --origin-port 39188 *> C:\QAInput\root-cause-checkpoint.log
    if ($LASTEXITCODE -ne 0) { throw 'Could not create paused test task' }
    $upgrade = Start-Process msiexec.exe -ArgumentList '/i "C:\QAInput\root-cause-candidate.msi" /qn /norestart /l*v "C:\QAInput\root-cause-upgrade-msi.log"' -Wait -PassThru -WindowStyle Hidden
    $result.upgrade_exit_code = $upgrade.ExitCode
    if ($upgrade.ExitCode -ne 0) { throw "Same-version upgrade failed: $($upgrade.ExitCode)" }
    $oldKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{9E013521-042C-429C-9FC7-7F09F6C964B9}'
    $newKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\' + $expected.product_code
    if ((Test-Path $oldKey) -or -not (Test-Path $newKey)) { throw 'Upgrade did not replace old MSI registration' }
    if ((Get-FileHash "$install\app\resources\extensions\HLSDownloader-7.0.3-Firefox.zip").Hash -ne $expected.firefox_sha256) { throw 'Upgrade kept the old Firefox archive' }
    $result.same_path_and_latest_extension = $true
    $result.old_registration_removed = $true
    & C:\QAPython\python.exe C:\QAInput\scripts\msi_checkpoint_fixture.py --mode verify-resume --engine "$install\app\resources\HLSDownloaderEngine.exe" --root C:\QAWork\UpgradeState --state C:\QAInput\root-cause-checkpoint.json --report C:\QAInput\root-cause-after-upgrade.json --core-port 39187 --origin-port 39188 *> C:\QAInput\root-cause-resume.log
    if ($LASTEXITCODE -ne 0) { throw 'Paused test task did not survive upgrade and finish with matching hash' }
    $result.paused_task_resumed_and_hash_verified = $true
    $uninstall = Start-Process msiexec.exe -ArgumentList ('/x ' + $expected.product_code + ' /qn /norestart /l*v "C:\QAInput\root-cause-upgrade-uninstall.log"') -Wait -PassThru -WindowStyle Hidden
    $result.uninstall_exit_code = $uninstall.ExitCode
    if ($uninstall.ExitCode -ne 0 -or (Test-Path "$install\HLSDownloader.exe") -or (Test-Path $newKey)) { throw 'Upgrade uninstall left product files or registration' }
    $result.passed = $true
} catch { $result.error = $_.Exception.Message }
$result | ConvertTo-Json -Depth 6 | Set-Content C:\QAInput\root-cause-upgrade-result.json -Encoding utf8
