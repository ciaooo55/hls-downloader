$ErrorActionPreference='Stop'
if($env:USERNAME -ne 'WDAGUtilityAccount'){throw 'Windows Sandbox only'}
New-Item -ItemType Directory -Force C:\QAWork,C:\QAWork\tmp | Out-Null
$env:TEMP='C:\QAWork\tmp';$env:TMP=$env:TEMP
. C:\QARunners\common.ps1
Copy-Item C:\QAPerf\python C:\QAWork\browser-python -Recurse
"python312.zip`n.`nC:\QAInput\pydeps`nC:\QAPerf\pydeps`nC:\QAScripts" | Set-Content C:\QAWork\browser-python\python312._pth -Encoding ASCII
try {
    Run C:\QAWork\browser-python\python.exe @('C:\QAScripts\smoke_v7_installed_footprint.py','--msi','C:\QAInput\candidate.msi','--manifest','C:\QAInput\installer.json','--expected-components','C:\QAInput\components.json','--idle-seconds','15','--report','C:\QAReports\render-installed-audit.json') 'render-installed-audit'
} catch { $_ | Out-String | Set-Content C:\QAReports\render-guest-error.txt; throw }
finally { 'finished' | Set-Content C:\QAReports\render-guest-finished.txt }
