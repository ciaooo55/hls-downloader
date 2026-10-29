$ErrorActionPreference='Stop'
if($env:USERNAME -ne 'WDAGUtilityAccount'){throw 'Windows Sandbox only'}
$report='C:\QACandidateReports'; $work='C:\QACandidateWork'; $install='C:\QACandidateInstalled'
New-Item -ItemType Directory -Force $work,"$work\tmp" | Out-Null
$env:TEMP="$work\tmp"; $env:TMP=$env:TEMP; $env:HLS_VM_ACCEPTANCE='1'; $env:PYTHONDONTWRITEBYTECODE='1'; $env:SLINT_BACKEND='winit-software'
$result=[ordered]@{passed=$false;finished=$false;phase='setup';scope='Installation, recovery, installed browser matrices and native input; performance thresholds reported separately';host_product_install_performed=$false;checks=@();native_input='pending'}
function Save {$result | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath "$report\candidate.json" -Encoding UTF8}
function Record($name,$actual){$result.checks+=@{name=$name;actual=$actual};$result.phase=$name;Save}
function Run([string]$exe,[string[]]$arguments,[string]$name,[int]$timeout=180000){
 $quoted=@($arguments | ForEach-Object {'"'+($_ -replace '(\\*)"','$1$1\"' -replace '(\\+)$','$1$1')+'"'})
 $p=Start-Process -FilePath $exe -ArgumentList $quoted -WindowStyle Hidden -PassThru -RedirectStandardOutput "$report\$name.stdout.txt" -RedirectStandardError "$report\$name.stderr.txt"
 [void]$p.Handle
 if(-not $p.WaitForExit($timeout)){Stop-Process -Id $p.Id -Force;throw "$name timeout"}
 if($p.ExitCode -ne 0){throw "$name exit $($p.ExitCode)"}
}
function Msi([string[]]$arguments,[string]$name,[int]$expected=0){
 $p=Start-Process msiexec.exe -ArgumentList ($arguments+@('/norestart','/l*v',"$report\$name.log")) -WindowStyle Hidden -PassThru
 [void]$p.Handle
 if(-not $p.WaitForExit(180000)){throw "$name timeout"}
 if($p.ExitCode -ne $expected){throw "$name exit $($p.ExitCode), expected $expected"}
}
function Stop-Runtime {Get-CimInstance Win32_Process | Where-Object {$_.ExecutablePath -and $_.ExecutablePath.StartsWith($install+'\',[StringComparison]::OrdinalIgnoreCase)} | ForEach-Object {Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue}}
function Checkpoint([string]$mode){Run C:\QAPython\python.exe @('C:\QAScripts\msi_checkpoint_fixture.py','--mode',$mode,'--engine',"$install\app\resources\HLSDownloaderEngine.exe",'--root',"$work\checkpoint",'--state',"$work\checkpoint\state.json",'--core-port','18671','--origin-port','18672','--report',"$report\checkpoint-$mode.json") "checkpoint-$mode"}
$installed=$null
$result.stage_errors=@()
try {
 $meta=Get-Content C:\QACandidateRunners\acceptance-input.json -Raw -Encoding UTF8 | ConvertFrom-Json
 $candidate=Join-Path 'C:\QACandidate' $meta.msi
 $result.candidate_msi_sha256=(Get-FileHash -LiteralPath $candidate).Hash.ToLowerInvariant()
 if($result.candidate_msi_sha256 -ne $meta.sha256){throw 'Candidate hash mismatch'}
 $wi=New-Object -ComObject WindowsInstaller.Installer
 $db=$wi.OpenDatabase($candidate,0);$view=$db.OpenView("SELECT `Value` FROM `Property` WHERE `Property` = 'ProductCode'");$view.Execute();$code=$view.Fetch().StringData(1)
 Add-Type -Namespace CandidateAudit -Name MSI -MemberDefinition '[System.Runtime.InteropServices.DllImport("msi.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)] public static extern int MsiQueryProductState(string product);' 
 Copy-Item -LiteralPath $candidate -Destination "$work\candidate.msi"
 Msi @('/i',"$work\candidate.msi",'/qn',"INSTALLDIR=$install") 'fresh-install';$installed=$code
 Record 'same-msi-fresh-install' $true
 $result.scope='Same MSI: remaining native input and targeted 12-connection I/O diagnosis only'
 try {Run C:\QAPython\python.exe @('C:\QACandidateRunners\candidate-transfer-probe.py','--engine',"$install\app\resources\HLSDownloaderEngine.exe",'--report',"$report\transfer-12-profile.json",'--stage-root',"$work\performance",'--connections','12','--profile-io') 'transfer-12-profile'} catch {$result.performance_error=$_.Exception.Message;Save}
 if(Test-Path "$report\transfer-12-profile.json"){$result.performance=Get-Content "$report\transfer-12-profile.json" -Raw -Encoding UTF8 | ConvertFrom-Json}
 Stop-Runtime
 Checkpoint 'create-pause';Checkpoint 'verify-resume'
 $env:HLS_V7_DATA_DIR="$work\checkpoint\data";$env:HLS_V7_DOWNLOAD_DIR="$work\checkpoint\downloads";Remove-Item Env:HLS_V7_PIPE -ErrorAction SilentlyContinue
 Run C:\QAPython\python.exe @('C:\QACandidateRunners\native-seed.py') 'native-seed'
 $env:HLS_V7_CORE_TCP='1';$env:HLS_V7_CORE_BIND='127.0.0.1:18679';$env:HLS_UI_TEST_API='1';$env:HLS_UI_TEST_TOKEN='candidate-local-20261029-ui';$env:HLS_UI_TEST_PORT='18744';$env:HLS_UI_AUDIT_WIDTH='1280';$env:HLS_UI_AUDIT_HEIGHT='760'
 Start-Process "$install\HLSDownloader.exe" -WorkingDirectory $install -WindowStyle Hidden | Out-Null
 $result.phase='ready-for-sandbox-native-input';Save
 # Host records physical actions separately and writes only this completion marker.
 $deadline=(Get-Date).AddMinutes(45)
 while(-not(Test-Path "$report\native-input-finished.json") -and (Get-Date) -lt $deadline){try{Invoke-RestMethod -Uri 'http://127.0.0.1:18744/state' -Headers @{'X-HLS-Test-Token'='candidate-local-20261029-ui'} -TimeoutSec 2 | ConvertTo-Json -Depth 18 | Set-Content "$report\native-state-latest.json" -Encoding UTF8}catch{};Start-Sleep -Seconds 2}
 if(-not(Test-Path "$report\native-input-finished.json")){throw 'Native input handoff timed out'}
 $result.native_input=Get-Content "$report\native-input-finished.json" -Raw -Encoding UTF8 | ConvertFrom-Json
 Run C:\QAPython\python.exe @('C:\QACandidateRunners\native-observe.py') 'native-final-observe'
 Stop-Runtime;Msi @('/x',$code,'/qn') 'fresh-uninstall';$installed=$null
 if([CandidateAudit.MSI]::MsiQueryProductState($code) -ne -1 -or (Test-Path "$install\HLSDownloader.exe")){throw 'Fresh uninstall incomplete'}
 if(-not(Test-Path "$work\checkpoint\downloads\fixture.bin")){throw 'Uninstall deleted user fixture output'}
 Record 'fresh-uninstall-preserves-task-output' $true
 $result.passed=$result.stage_errors.Count -eq 0 -and $result.native_input.passed
}catch{$result.error=$_.Exception.Message;$result.position=$_.InvocationInfo.PositionMessage}
finally {try{if($installed){Stop-Runtime;Msi @('/x',$installed,'/qn') 'failure-cleanup-uninstall'}}catch{$result.cleanup_error=$_.Exception.Message};$result.finished=$true;Save}
