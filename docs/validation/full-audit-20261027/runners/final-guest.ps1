$ErrorActionPreference='Stop'
if($env:USERNAME -ne 'WDAGUtilityAccount'){throw 'Windows Sandbox only'}
[IO.File]::WriteAllText('C:\QAReports\final-guest-started.txt',[DateTime]::UtcNow.ToString('o'))
New-Item -ItemType Directory -Force C:\QAWork,C:\QAWork\tmp | Out-Null
$env:TEMP='C:\QAWork\tmp';$env:TMP=$env:TEMP
. C:\QARunners\common.ps1
Copy-Item C:\QAPerf\python C:\QAWork\browser-python -Recurse
"python312.zip`n.`nC:\QAInput\pydeps`nC:\QAPerf\pydeps`nC:\QAScripts" | Set-Content C:\QAWork\browser-python\python312._pth -Encoding ASCII
try { . C:\QARunners\installed.ps1 }
catch { $_ | Out-String | Set-Content C:\QAReports\final-guest-error.txt; throw }
finally { 'finished' | Set-Content C:\QAReports\final-guest-finished.txt }
