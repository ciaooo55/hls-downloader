$ErrorActionPreference='Stop'
if($env:USERNAME -ne 'WDAGUtilityAccount'){throw 'Windows Sandbox only'}
New-Item -ItemType Directory -Force C:\QAWork,C:\QAWork\tmp | Out-Null
$env:TEMP='C:\QAWork\tmp';$env:TMP=$env:TEMP
. C:\QARunners\common.ps1
Copy-Item C:\QAPerf\python C:\QAWork\python -Recurse
"python312.zip`n.`nC:\QAPerf\pydeps`nC:\QAScripts" | Set-Content C:\QAWork\python\python312._pth -Encoding ASCII
$meta=Get-Content C:\QAInput\installer.json -Raw | ConvertFrom-Json
$result=[ordered]@{passed=$false;candidate_sha256=$meta.sha256;checks=@()}
$installed=$false
function Msi([string[]]$arguments,[string]$name){
    $p=Start-Process msiexec.exe -ArgumentList ($arguments+@('/norestart','/l*v',"C:\QAReports\$name.log")) -WindowStyle Hidden -PassThru
    [void]$p.Handle
    if(-not $p.WaitForExit(180000)){throw "$name timed out"}
    if($p.ExitCode -ne 0){throw "$name exit $($p.ExitCode)"}
}
try {
    if((Get-FileHash C:\QAInput\candidate.msi).Hash.ToLowerInvariant() -ne $meta.sha256){throw 'Candidate hash mismatch'}
    Copy-Item C:\QAInput\candidate.msi C:\QAWork\candidate.msi
    Msi @('/i','C:\QAWork\candidate.msi','/qn','INSTALLDIR="C:\QA Workbench"') 'workbench-install'
    $installed=$true
    $components=Get-Content C:\QAInput\components.json -Raw | ConvertFrom-Json
    foreach($p in $components.PSObject.Properties){if((Get-FileHash (Join-Path 'C:\QA Workbench' $p.Name)).Hash.ToLowerInvariant() -ne $p.Value){throw "Payload mismatch: $($p.Name)"}}
    $result.checks+=@{name='exact installed component hashes';count=@($components.PSObject.Properties).Count}
    Run C:\QAWork\python\python.exe @('C:\QARunners\workbench-probe.py') 'workbench-probe'
    $result.checks+=@{name='real downloads, settings restart and light/dark paint captures';passed=$true}
    $result.passed=$true
} catch {$result.error=$_.Exception.Message;$result.position=$_.InvocationInfo.PositionMessage;throw}
finally {
    Get-CimInstance Win32_Process | Where-Object {$_.ExecutablePath -and $_.ExecutablePath.StartsWith('C:\QA Workbench\',[StringComparison]::OrdinalIgnoreCase)} | ForEach-Object {Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue}
    if($installed){Msi @('/x',$meta.product_code,'/qn') 'workbench-uninstall'; $result.uninstalled=(-not(Test-Path 'C:\QA Workbench\HLSDownloader.exe'))}
    $result | ConvertTo-Json -Depth 8 | Set-Content C:\QAReports\installed-workbench.json -Encoding UTF8
    'finished' | Set-Content C:\QAReports\workbench-finished.txt
}
