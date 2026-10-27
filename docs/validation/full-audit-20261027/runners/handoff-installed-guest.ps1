$ErrorActionPreference='Stop'
if($env:USERNAME -ne 'WDAGUtilityAccount'){throw 'Windows Sandbox only'}
New-Item -ItemType Directory -Force C:\QAWork,C:\QAWork\tmp | Out-Null
$env:TEMP='C:\QAWork\tmp';$env:TMP=$env:TEMP
. C:\QARunners\common.ps1
Copy-Item C:\QAPerf\python C:\QAWork\python -Recurse
"python312.zip`n.`nC:\QAScripts" | Set-Content C:\QAWork\python\python312._pth -Encoding ASCII
Add-Type -Namespace HistoryAudit -Name MSI -MemberDefinition '[System.Runtime.InteropServices.DllImport("msi.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)] public static extern int MsiQueryProductState(string product);'
$meta=Get-Content C:\QAInput\installer.json -Raw | ConvertFrom-Json
$old=Get-Content C:\QABaseline\installer.json -Raw | ConvertFrom-Json
$install='C:\QA History Upgrade'
$result=[ordered]@{passed=$false;candidate_sha256=$meta.sha256;checks=@();scope='Installed payload, running upgrade, paused checkpoint, large handoff history and uninstall; no desktop input or full button acceptance'}
$installed=$null
function Save {$result | ConvertTo-Json -Depth 8 | Set-Content C:\QAReports\installed-history.json -Encoding UTF8}
function Record($name,$actual){$result.checks+=@{name=$name;actual=$actual};Save}
function Msi([string[]]$arguments,[string]$name){
    $p=Start-Process msiexec.exe -ArgumentList ($arguments+@('/norestart','/l*v',"C:\QAReports\$name.log")) -WindowStyle Hidden -PassThru
    [void]$p.Handle
    if(-not $p.WaitForExit(180000)){throw "$name timed out"}
    if($p.ExitCode -ne 0){throw "$name exit $($p.ExitCode)"}
}
function Stop-Runtime {
    Get-CimInstance Win32_Process | Where-Object {$_.ExecutablePath -and $_.ExecutablePath.StartsWith($install+'\',[StringComparison]::OrdinalIgnoreCase)} | ForEach-Object {Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue}
}
function Checkpoint([string]$mode){
    Run C:\QAWork\python\python.exe @('C:\QAScripts\msi_checkpoint_fixture.py','--mode',$mode,'--engine',"$install\app\resources\HLSDownloaderEngine.exe",'--root','C:\QAWork\installed-checkpoint','--state','C:\QAWork\installed-checkpoint\state.json','--core-port','18671','--origin-port','18672','--report',"C:\QAReports\history-$mode.json") "history-$mode"
}
try {
    if((Get-FileHash C:\QAInput\candidate.msi).Hash.ToLowerInvariant() -ne $meta.sha256){throw 'Candidate hash mismatch'}
    if((Get-FileHash C:\QAInput\baseline.msi).Hash.ToLowerInvariant() -ne $old.sha256){throw 'Baseline hash mismatch'}
    Copy-Item C:\QAInput\baseline.msi C:\QAWork\baseline.msi
    Copy-Item C:\QAInput\candidate.msi C:\QAWork\candidate.msi
    Msi @('/i','C:\QAWork\baseline.msi','/qn',('INSTALLDIR="'+$install+'"')) 'history-baseline-install'
    $installed=$old.product_code
    Checkpoint 'create-pause'
    $env:HLS_V7_DATA_DIR='C:\QAWork\installed-checkpoint\data'
    $env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\installed-checkpoint\downloads'
    Start-Process "$install\app\resources\HLSDownloaderEngine.exe" -WorkingDirectory $install -WindowStyle Hidden | Out-Null
    Start-Process "$install\HLSDownloader.exe" -WorkingDirectory $install -WindowStyle Hidden | Out-Null
    Msi @('/i','C:\QAWork\candidate.msi','/qn') 'history-running-upgrade'
    $installed=$meta.product_code
    Stop-Runtime
    if([HistoryAudit.MSI]::MsiQueryProductState($old.product_code) -ne -1 -or [HistoryAudit.MSI]::MsiQueryProductState($meta.product_code) -ne 5){throw 'Upgrade registration mismatch'}
    $components=Get-Content C:\QAInput\components.json -Raw | ConvertFrom-Json
    foreach($p in $components.PSObject.Properties){if((Get-FileHash (Join-Path $install $p.Name)).Hash.ToLowerInvariant() -ne $p.Value){throw "Payload mismatch: $($p.Name)"}}
    Record 'running r19 upgrade, custom path and exact installed component hashes' @($components.PSObject.Properties).Count
    Checkpoint 'verify-resume'
    Record 'paused task survives and resumes to original SHA256' $true
    Run C:\QAWork\python\python.exe @('C:\QAScripts\smoke_v7_handoff_history.py','--runtime',"$install\app\resources",'--report','C:\QAReports\installed-large-history.json') 'installed-large-history'
    Record 'actual installed Core and Native Host work with history beyond 4MiB' $true
    Run "$install\app\resources\HLSDownloaderPresenter.exe" @('--self-test') 'installed-presenter-self-test'
    Record 'installed Presenter constructs its confirmation component' $true
    Msi @('/x',$meta.product_code,'/qn') 'history-uninstall'
    $installed=$null
    if([HistoryAudit.MSI]::MsiQueryProductState($meta.product_code) -ne -1 -or (Test-Path "$install\HLSDownloader.exe")){throw 'Uninstall incomplete'}
    Record 'candidate MSI registration and launcher removed' $true
    $result.passed=$true
} catch {$result.error=$_.Exception.Message;$result.position=$_.InvocationInfo.PositionMessage;throw}
finally {
    if($installed){Stop-Runtime;Msi @('/x',$installed,'/qn') 'history-final-uninstall'}
    Save
    'finished' | Set-Content C:\QAReports\installed-history-finished.txt
}
