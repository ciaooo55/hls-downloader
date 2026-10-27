. C:\QARunners\common.ps1
Add-Type -Namespace Audit -Name MSI -MemberDefinition '[System.Runtime.InteropServices.DllImport("msi.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)] public static extern int MsiQueryProductState(string product);'
$meta=Get-Content C:\QAInput\installer.json -Raw | ConvertFrom-Json
$old=Get-Content C:\QAInput\baseline-installer.json -Raw | ConvertFrom-Json
$install='C:\QAUpgradeAudit'
if(Test-Path -LiteralPath $install){throw 'Fresh audit install directory required'}
$result=[ordered]@{passed=$false;candidate_sha256=$meta.sha256;checks=@();native_host_latency_gate='Separate earlier failure retained; not passed by this lifecycle run'}
function Save { $result | ConvertTo-Json -Depth 8 | Set-Content C:\QAReports\installed-audit.json -Encoding UTF8 }
function Record($name,$actual) { $result.checks+=@{name=$name;actual=$actual};Save }
function Msi([string[]]$arguments,[string]$name,[int]$expected=0) {
    $p=Start-Process msiexec.exe -ArgumentList ($arguments+@('/norestart','/l*v',"C:\QAReports\$name.log")) -WindowStyle Hidden -PassThru
    [void]$p.Handle
    if(-not $p.WaitForExit(180000)){throw "$name timed out"}
    if($p.ExitCode -ne $expected){throw "$name exit $($p.ExitCode), expected $expected"}
}
function Stop-Runtime([string]$path) {
    Get-CimInstance Win32_Process | Where-Object {$_.ExecutablePath -and $_.ExecutablePath.StartsWith($path+'\',[StringComparison]::OrdinalIgnoreCase)} | ForEach-Object {Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue}
}
function Checkpoint([string]$mode) {
    Run C:\QAWork\browser-python\python.exe @('C:\QAScripts\msi_checkpoint_fixture.py','--mode',$mode,'--engine',"$install\app\resources\HLSDownloaderEngine.exe",'--root','C:\QAWork\installed-checkpoint','--state','C:\QAWork\installed-checkpoint\state.json','--core-port','18671','--origin-port','18672','--report',"C:\QAReports\installed-$mode.json") "installed-$mode"
}
$installed=$null
try {
    Stop-Runtime 'C:\QAWork\runtime'
    if((Get-FileHash C:\QAInput\candidate.msi).Hash.ToLowerInvariant() -ne $meta.sha256){throw 'Candidate hash mismatch'}
    Copy-Item C:\QAInput\baseline.msi C:\QAWork\audit-baseline.msi
    Copy-Item C:\QAInput\candidate.msi C:\QAWork\audit-candidate.msi
    Msi @('/i','C:\QAWork\audit-baseline.msi','/qn',"INSTALLDIR=$install") 'audit-baseline-install'
    $installed=$old.product_code
    Checkpoint 'create-pause'
    $before=(Get-FileHash C:\QAWork\installed-checkpoint\data\data.db).Hash
    $oldEngine=(Get-FileHash "$install\app\resources\HLSDownloaderEngine.exe").Hash
    # Reuse the exact existing failure injector without executing the lifecycle runner.
    $tokens=$null;$errors=$null
    $ast=[Management.Automation.Language.Parser]::ParseFile('C:\QAScripts\verify-v7-msi-lifecycle.ps1',[ref]$tokens,[ref]$errors)
    $fn=$ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Add-DeferredFailure'},$true)
    if(@($fn).Count -ne 1 -or $errors.Count){throw 'Failure injector is not uniquely parsable'}
    Invoke-Expression $fn.Extent.Text
    Copy-Item C:\QAInput\candidate.msi C:\QAWork\forced-audit-rollback.msi
    Add-DeferredFailure C:\QAWork\forced-audit-rollback.msi
    Msi @('/i','C:\QAWork\forced-audit-rollback.msi','/qn',"INSTALLDIR=$install") 'audit-forced-rollback' 1603
    if([Audit.MSI]::MsiQueryProductState($old.product_code) -ne 5 -or [Audit.MSI]::MsiQueryProductState($meta.product_code) -eq 5){throw 'Rollback registration mismatch'}
    if((Get-FileHash "$install\app\resources\HLSDownloaderEngine.exe").Hash -ne $oldEngine -or (Get-FileHash C:\QAWork\installed-checkpoint\data\data.db).Hash -ne $before){throw 'Rollback modified old Engine/database'}
    Checkpoint 'verify-paused'
    Record 'forced post-file-install failure restores old registration, Engine and paused task' $true
    $env:HLS_V7_DATA_DIR='C:\QAWork\installed-checkpoint\data'
    $env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\installed-checkpoint\downloads'
    Start-Process "$install\app\resources\HLSDownloaderEngine.exe" -WorkingDirectory $install -WindowStyle Hidden | Out-Null
    Start-Process "$install\HLSDownloader.exe" -WorkingDirectory $install -WindowStyle Hidden | Out-Null
    Msi @('/i','C:\QAWork\audit-candidate.msi','/qn') 'audit-running-upgrade'
    $installed=$meta.product_code
    Stop-Runtime $install
    if([Audit.MSI]::MsiQueryProductState($old.product_code) -ne -1 -or [Audit.MSI]::MsiQueryProductState($meta.product_code) -ne 5){throw 'Upgrade registration mismatch'}
    $components=Get-Content C:\QAInput\components.json -Raw | ConvertFrom-Json
    foreach($p in $components.PSObject.Properties){if((Get-FileHash (Join-Path $install $p.Name)).Hash.ToLowerInvariant() -ne $p.Value){throw "Payload mismatch: $($p.Name)"}}
    Record 'running old Core/UI upgrade to same candidate payload; old registration removed' @($components.PSObject.Properties).Count
    Checkpoint 'verify-resume'
    Record 'paused task survives upgrade and resumes to original SHA256' $true
    Msi @('/x',$meta.product_code,'/qn') 'audit-candidate-uninstall'
    $installed=$null
    if([Audit.MSI]::MsiQueryProductState($meta.product_code) -ne -1 -or (Test-Path "$install\HLSDownloader.exe")){throw 'Upgrade uninstall incomplete'}
    Remove-Item Env:HLS_V7_DATA_DIR,Env:HLS_V7_DOWNLOAD_DIR -ErrorAction SilentlyContinue
    Run C:\QAWork\browser-python\python.exe @('C:\QAScripts\smoke_v7_installed_footprint.py','--msi','C:\QAInput\candidate.msi','--manifest','C:\QAInput\installer.json','--expected-components','C:\QAInput\components.json','--idle-seconds','60','--verify-http-retry','--verify-hls-retry','--report','C:\QAReports\fresh-installed-audit.json') 'fresh-installed-audit'
    Record 'fresh custom path, KnownFolder Downloads, installed retry/cleanup, UI connection, 60s resources and uninstall' $true
    $result.passed=$true
} catch {$result.error=$_.Exception.Message;$result.position=$_.InvocationInfo.PositionMessage;throw}
finally {if($installed){Stop-Runtime $install;Msi @('/x',$installed,'/qn') 'audit-final-uninstall'};Save}
