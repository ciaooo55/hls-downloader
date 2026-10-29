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
 Record 'prior-same-msi-selftest-and-performance' 'candidate-remaining: 1 connection 34.19MiB/s pass; 12 connections 12.19MiB/s fails 20MiB/s gate'
 New-Item -ItemType Directory -Force C:\QACurrentWork | Out-Null
 # Use actual installed Core/Host/Presenter and archives for both browser matrices.
 $env:HLS_CURRENT_RESOURCES="$install\app\resources"
 foreach($item in @(@('Chromium','chrome-mv3'),@('Firefox','firefox-mv3'))){Run C:\QAPython\python.exe @('-m','zipfile','-e', "$install\app\resources\extensions\HLSDownloader-7.0.3-$($item[0]).zip","C:\QACurrentExtensions\$($item[1])") ('extract-'+$item[0])}
 $env:HLS_REPLY_REAL_HOST="$install\app\resources\HLSDownloaderNativeHost.exe"
 $env:HLS_REPLY_PROXY_PATH="$work\NativeReplyProxy.exe"
 Add-Type -Path C:\QACandidateRunners\NativeReplyProxy.cs -ReferencedAssemblies System.Web.Extensions,System.Core -OutputAssembly $env:HLS_REPLY_PROXY_PATH -OutputType ConsoleApplication
 foreach($family in @('edge','firefox')){
  $env:HLS_V7_DATA_DIR="$work\browser-$family-data";$env:HLS_V7_DOWNLOAD_DIR="$work\browser-$family-downloads"
  $env:HLS_V7_PIPE="\\.\pipe\HLSDownloader.candidate-$family"
  $env:HLS_CURRENT_REPORT_DIR="$report\$family";$env:HLS_REPLY_PROXY_STATE="$report\$family\proxy";$env:HLS_REPLY_FAILURE_COUNT='1'
  New-Item -ItemType Directory -Force $env:HLS_CURRENT_REPORT_DIR,$env:HLS_REPLY_PROXY_STATE | Out-Null
  $browserArgs=@('C:\QACandidateRunners\current-browser-guest.mjs');if($family -eq 'firefox'){$browserArgs+='--firefox'}
  Run C:\QACurrentInput\node.exe $browserArgs "$family-matrix" 240000
  $browserResult=Get-Content "$report\$family\$family.json" -Raw -Encoding UTF8 | ConvertFrom-Json
  if(-not $browserResult.passed){throw "$family matrix failed"}
  Record "installed-$family-handoff-matrix" $browserResult.checks.Count
  $env:HLS_CURRENT_REPORT_DIR="$report\$family\initial-offline";New-Item -ItemType Directory -Force $env:HLS_CURRENT_REPORT_DIR | Out-Null
  Run C:\QACurrentInput\node.exe ($browserArgs+'--offline-only') "$family-offline" 90000
  Record "installed-$family-initial-offline" $true
 }
 Stop-Runtime
 Checkpoint 'create-pause';Checkpoint 'verify-resume'
 $env:HLS_V7_DATA_DIR="$work\checkpoint\data";$env:HLS_V7_DOWNLOAD_DIR="$work\checkpoint\downloads";Remove-Item Env:HLS_V7_PIPE -ErrorAction SilentlyContinue
 Run C:\QAPython\python.exe @('C:\QACandidateRunners\native-seed.py') 'native-seed'
 $env:HLS_UI_TEST_API='1';$env:HLS_UI_TEST_TOKEN='candidate-local-20261029-ui';$env:HLS_UI_TEST_PORT='18744';$env:HLS_UI_AUDIT_WIDTH='1280';$env:HLS_UI_AUDIT_HEIGHT='760'
 Start-Process "$install\HLSDownloader.exe" -WorkingDirectory $install -WindowStyle Hidden | Out-Null
 $result.phase='ready-for-sandbox-native-input';Save
 # Host records physical actions separately and writes only this completion marker.
 $deadline=(Get-Date).AddMinutes(30)
 while(-not(Test-Path "$report\native-input-finished.json") -and (Get-Date) -lt $deadline){try{Invoke-RestMethod -Uri 'http://127.0.0.1:18744/state' -Headers @{'X-HLS-Test-Token'='candidate-local-20261029-ui'} -TimeoutSec 2 | ConvertTo-Json -Depth 18 | Set-Content "$report\native-state-latest.json" -Encoding UTF8}catch{};Start-Sleep -Seconds 2}
 if(-not(Test-Path "$report\native-input-finished.json")){throw 'Native input handoff timed out'}
 $result.native_input=Get-Content "$report\native-input-finished.json" -Raw -Encoding UTF8 | ConvertFrom-Json
 Stop-Runtime;Msi @('/x',$code,'/qn') 'fresh-uninstall';$installed=$null
 if([CandidateAudit.MSI]::MsiQueryProductState($code) -ne -1 -or (Test-Path "$install\HLSDownloader.exe")){throw 'Fresh uninstall incomplete'}
 if(-not(Test-Path "$work\checkpoint\downloads\fixture.bin")){throw 'Uninstall deleted user fixture output'}
 Record 'fresh-uninstall-preserves-task-output' $true
 $result.passed=$true
}catch{$result.error=$_.Exception.Message;$result.position=$_.InvocationInfo.PositionMessage}
finally {try{if($installed){Stop-Runtime;Msi @('/x',$installed,'/qn') 'failure-cleanup-uninstall'}}catch{$result.cleanup_error=$_.Exception.Message};$result.finished=$true;Save}
