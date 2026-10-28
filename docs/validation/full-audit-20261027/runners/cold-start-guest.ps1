$ErrorActionPreference='Stop'
if($env:USERNAME -ne 'WDAGUtilityAccount'){throw 'Windows Sandbox only'}
New-Item -ItemType Directory -Force C:\QAWork,C:\QAWork\tmp | Out-Null
$env:TEMP='C:\QAWork\tmp';$env:TMP=$env:TEMP
. C:\QARunners\common.ps1
Copy-Item C:\QAPerf\python C:\QAWork\python -Recurse
"python312.zip`n.`nC:\QAPerf\pydeps`nC:\QAScripts" | Set-Content C:\QAWork\python\python312._pth -Encoding ASCII
$meta=Get-Content C:\QAInput\installer.json -Raw | ConvertFrom-Json
$result=[ordered]@{passed=$false;msi_sha256=$meta.sha256;install_exit=$null;startup_gate_passed=$false;uninstall_exit=$null}
$installed=$false
try {
    if((Get-FileHash C:\QAInput\candidate.msi).Hash.ToLowerInvariant() -ne $meta.sha256){throw 'Candidate MSI mismatch'}
    Copy-Item C:\QAInput\candidate.msi C:\QAWork\candidate.msi
    $p=Start-Process msiexec.exe -ArgumentList @('/i','C:\QAWork\candidate.msi','/qn','INSTALLDIR="C:\QA Cold Startup"','/norestart','/l*v','C:\QAReports\cold-install.log') -WindowStyle Hidden -PassThru
    [void]$p.Handle
    if(-not $p.WaitForExit(180000)){throw 'Install timeout'}
    $result.install_exit=$p.ExitCode
    if($p.ExitCode -ne 0){throw "Install exit $($p.ExitCode)"}
    $installed=$true
    Run C:\QAWork\python\python.exe @('C:\QARunners\cold-start.py') 'cold-start'
    $result.startup_gate_passed=$true
} catch {$result.error=$_.Exception.Message}
finally {
    if($installed){
        $p=Start-Process msiexec.exe -ArgumentList @('/x',$meta.product_code,'/qn','/norestart','/l*v','C:\QAReports\cold-uninstall.log') -WindowStyle Hidden -PassThru
        [void]$p.Handle
        if($p.WaitForExit(180000)){$result.uninstall_exit=$p.ExitCode}else{$result.error='Uninstall timeout'}
    }
    $result.passed=($result.startup_gate_passed -and $result.uninstall_exit -eq 0)
    $result | ConvertTo-Json -Depth 6 | Set-Content C:\QAReports\cold-installed.json -Encoding UTF8
    'finished' | Set-Content C:\QAReports\cold-finished.txt
}
