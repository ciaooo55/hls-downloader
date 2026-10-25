$ErrorActionPreference='Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
$env:HLS_VM_ACCEPTANCE='1'
New-Item -ItemType Directory -Force -Path C:\QAWork | Out-Null
$process=Start-Process -FilePath C:\QAInput\node.exe -ArgumentList C:\QAInput\installer-qa.cjs -WindowStyle Hidden -PassThru -RedirectStandardOutput C:\QAReports\installer.stdout.txt -RedirectStandardError C:\QAReports\installer.stderr.txt
[void]$process.Handle
if (-not $process.WaitForExit(600000)) { Stop-Process -Id $process.Id -Force; throw 'Installer QA timed out' }
$process.ExitCode | Set-Content C:\QAReports\installer.exit.txt
