$ErrorActionPreference='Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
$env:HLS_VM_ACCEPTANCE='1'
$env:PYTHONDONTWRITEBYTECODE='1'
$env:TEMP='C:\QAWork\tmp'
$env:TMP=$env:TEMP
New-Item -ItemType Directory -Force C:\QAWork,$env:TEMP | Out-Null
Set-Location C:\QAWork
$result=[ordered]@{passed=$false;finished=$false;current='setup'}
function Save { $result | ConvertTo-Json -Depth 6 | Set-Content C:\QAReports\summary.json -Encoding UTF8 }
try {
    Save
    Copy-Item C:\QAPerf\python C:\QAWork\python -Recurse
    "python312.zip`n.`nC:\QAInput\pydeps`nC:\QAPerf\pydeps`nC:\QAScripts" | Set-Content C:\QAWork\python\python312._pth -Encoding ASCII
    $result.current='fresh-installed';Save
    $p=Start-Process C:\QAWork\python\python.exe -ArgumentList @('C:\QAScripts\smoke_v7_installed_footprint.py','--msi','C:\QAFollowup\delivery\candidate.msi','--manifest','C:\QAFollowup\delivery\installer.json','--expected-components','C:\QAFollowup\delivery\components.json','--verify-native-host-startup','--report','C:\QAReports\fresh-installed.json') -WindowStyle Hidden -PassThru -RedirectStandardOutput C:\QAReports\fresh-installed.stdout.txt -RedirectStandardError C:\QAReports\fresh-installed.stderr.txt
    [void]$p.Handle
    if (-not $p.WaitForExit(600000)) { Stop-Process -Id $p.Id -Force;throw 'fresh-installed timeout' }
    if ($p.ExitCode -ne 0) { throw "fresh-installed failed: $($p.ExitCode)" }
    $result.passed=$true
} catch {$result.error=$_.Exception.Message;$result.position=$_.InvocationInfo.PositionMessage}
finally {$result.finished=$true;Save}