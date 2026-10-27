$ErrorActionPreference='Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
Add-Type -Namespace QA -Name MSI -MemberDefinition '[System.Runtime.InteropServices.DllImport("msi.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)] public static extern int MsiQueryProductState(string product);'
$meta=Get-Content C:\QAInput\installer.json -Raw | ConvertFrom-Json
$old=Get-Content C:\QAInput\baseline-installer.json -Raw | ConvertFrom-Json
$result=[ordered]@{passed=$false;checks=@()}
function Save { $result | ConvertTo-Json -Depth 7 | Set-Content C:\QAReports\installer.json -Encoding UTF8 }
function Record($name,$actual) { $result.checks+=@{name=$name;actual=$actual};Save }
function Run([string]$exe,[string[]]$arguments,[string]$name,[bool]$expectFailure=$false) {
    $p=Start-Process $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "C:\QAReports\$name.stdout.txt" -RedirectStandardError "C:\QAReports\$name.stderr.txt"
    [void]$p.Handle
    if(-not $p.WaitForExit(600000)){Stop-Process -Id $p.Id -Force;throw "$name timeout"}
    if(($expectFailure -and $p.ExitCode -eq 0) -or (-not $expectFailure -and $p.ExitCode -ne 0)){throw "$name unexpected exit $($p.ExitCode)"}
}
$installed=$null
try {
    Run C:\QAPerf\python\python.exe @('C:\QAScripts\smoke_v7_task_import.py','--engine','C:\QAInput\pre-portable-engine.exe','--report','C:\QAReports\before-portable-actual.json') 'before-portable-actual' $true
    $before=Get-Content C:\QAReports\before-portable-actual.json -Raw | ConvertFrom-Json
    if (-not ($before.checks | Where-Object {$_.name -eq 'reimport-diagnostics' -and $_.created_count -eq 0 -and $_.errors.message -match '下载目录必须位于默认下载根目录内'})) {throw 'Old code did not reproduce the expected cross-root import rejection'}
    Record 'old code rejects its own export from another download root' $true
    Copy-Item C:\QAInput\baseline.msi C:\QAWork\baseline.msi
    Copy-Item C:\QAInput\candidate.msi C:\QAWork\candidate.msi
    if((Get-FileHash C:\QAWork\candidate.msi).Hash.ToLowerInvariant() -ne $meta.sha256){throw 'MSI hash mismatch'}
    Run msiexec.exe @('/i','C:\QAWork\baseline.msi','/qn','INSTALLDIR=C:\QAUpgrade','/l*v','C:\QAReports\baseline-install.log') 'baseline-install'
    $installed=$old.product_code
    Start-Process C:\QAUpgrade\app\resources\HLSDownloaderEngine.exe -WorkingDirectory C:\QAUpgrade -WindowStyle Hidden | Out-Null
    Run msiexec.exe @('/i','C:\QAWork\candidate.msi','/qn','/l*v','C:\QAReports\candidate-upgrade.log') 'candidate-upgrade'
    $installed=$meta.product_code
    if([QA.MSI]::MsiQueryProductState($old.product_code) -ne -1 -or [QA.MSI]::MsiQueryProductState($meta.product_code) -ne 5){throw 'Upgrade registration mismatch'}
    Record 'r17 running Core upgrades, old MSI registration removed' $true
    $components=Get-Content C:\QAInput\components.json -Raw | ConvertFrom-Json
    foreach($property in $components.PSObject.Properties){if((Get-FileHash (Join-Path C:\QAUpgrade $property.Name)).Hash.ToLowerInvariant() -ne $property.Value){throw "Installed payload mismatch: $($property.Name)"}}
    Record 'all installed components match candidate' @($components.PSObject.Properties).Count
    Run C:\QAPerf\python\python.exe @('C:\QAScripts\smoke_v7_task_import.py','--engine','C:\QAUpgrade\app\resources\HLSDownloaderEngine.exe','--report','C:\QAReports\installed-task-import.json') 'installed-task-import'
    Record 'actual installed Engine imports, exports and downloads after partial failure' $true
    Run msiexec.exe @('/x',$meta.product_code,'/qn','/l*v','C:\QAReports\candidate-uninstall.log') 'candidate-uninstall'
    $installed=$null
    if([QA.MSI]::MsiQueryProductState($meta.product_code) -ne -1 -or (Test-Path C:\QAUpgrade\HLSDownloader.exe)){throw 'Uninstall incomplete'}
    Record 'upgrade candidate uninstalls' $true
    Run C:\QAPerf\python\python.exe @('C:\QAScripts\smoke_v7_installed_footprint.py','--msi','C:\QAInput\candidate.msi','--manifest','C:\QAInput\installer.json','--expected-components','C:\QAInput\components.json','--core-only','--verify-native-host-startup','--report','C:\QAReports\fresh-installed.json') 'fresh-installed'
    Record 'fresh custom path, Known Folder, public file, Native Host startup and uninstall' $true
    $result.passed=$true
} catch {$result.error=$_.Exception.Message;$result.position=$_.InvocationInfo.PositionMessage;throw}
finally {if($installed){Start-Process msiexec.exe -ArgumentList @('/x',$installed,'/qn') -WindowStyle Hidden -Wait};Save}
