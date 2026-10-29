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
 $old=@(Get-ChildItem C:\QAExistingDelivery -Filter 'HLSDownloader-7.0.3-20261028-*.msi' -File)
 if($old.Count -ne 1 -or (Get-FileHash -LiteralPath $old[0].FullName).Hash.ToLowerInvariant() -ne '8945ab49b5a77dac24f689cd80565184956345f2c8d8cc52c1b58426c7b70bd8'){throw 'Baseline hash mismatch'}
 $tokens=$null;$errors=$null
 $ast=[Management.Automation.Language.Parser]::ParseInput([IO.File]::ReadAllText('C:\QAScripts\verify-v7-msi-lifecycle.ps1',[Text.Encoding]::UTF8),[ref]$tokens,[ref]$errors)
 if($errors.Count){throw 'Lifecycle helpers not parsable'}
 foreach($name in @('Get-MsiProperty','Add-DeferredFailure')){
  $fn=@($ast.FindAll({param($node)$node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name},$true))
  if($fn.Count -ne 1){throw "Lifecycle helper not unique: $name"};Invoke-Expression $fn[0].Extent.Text
 }
 Add-Type -Namespace CandidateAudit -Name MSI -MemberDefinition '[System.Runtime.InteropServices.DllImport("msi.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)] public static extern int MsiQueryProductState(string product);'
 $oldCode=Get-MsiProperty $old[0].FullName 'ProductCode';$code=Get-MsiProperty $candidate 'ProductCode'
 if($oldCode -eq $code){throw 'Candidate must have a new ProductCode'}
 Copy-Item -LiteralPath $old[0].FullName -Destination "$work\baseline.msi"
 Copy-Item -LiteralPath $candidate -Destination "$work\candidate.msi"
 Msi @('/a',"$work\candidate.msi",'/qn',"TARGETDIR=$work\extracted") 'candidate-administrative-extract'
 $payload=@(Get-ChildItem "$work\extracted" -Filter HLSDownloader.exe -Recurse -File)
 if($payload.Count -ne 1){throw 'Extracted candidate launcher not unique'}
 $expected=@{}
 foreach($jar in Get-ChildItem (Join-Path $payload[0].DirectoryName 'app') -Filter '*.jar'){$expected['app\'+$jar.Name]=(Get-FileHash -LiteralPath $jar.FullName).Hash.ToLowerInvariant()}
 $result.expected_msi_jars=$expected
 $expected | ConvertTo-Json | Set-Content "$report\expected-msi-jars.json" -Encoding UTF8
 foreach($component in $meta.components.PSObject.Properties){$expected[$component.Name]=$component.Value}
 Msi @('/i',"$work\baseline.msi",'/qn',"INSTALLDIR=$install") 'baseline-install';$installed=$oldCode
 $result.baseline_performance=Get-Content C:\QACandidateRunners\baseline-transfer-12.json -Raw -Encoding UTF8 | ConvertFrom-Json; $result.baseline_performance_reused_from='candidate-reports/baseline-transfer-12.json'
 Checkpoint 'create-pause'
 Record 'rollback-evidence-reused-same-msi' 'candidate-lifecycle/candidate.json: forced-rollback-preserves-old-binary-database-paused-task'
 $env:HLS_V7_DATA_DIR="$work\checkpoint\data";$env:HLS_V7_DOWNLOAD_DIR="$work\checkpoint\downloads"
 Start-Process "$install\app\resources\HLSDownloaderEngine.exe" -WorkingDirectory $install -WindowStyle Hidden | Out-Null
 Start-Process "$install\HLSDownloader.exe" -WorkingDirectory $install -WindowStyle Hidden | Out-Null
 Msi @('/i',"$work\candidate.msi",'/qn') 'running-upgrade';$installed=$code;Stop-Runtime
 if([CandidateAudit.MSI]::MsiQueryProductState($oldCode) -ne -1 -or [CandidateAudit.MSI]::MsiQueryProductState($code) -ne 5){throw 'Upgrade registration mismatch'}
 foreach($component in $expected.GetEnumerator()){if((Get-FileHash -LiteralPath (Join-Path $install $component.Key)).Hash.ToLowerInvariant() -ne $component.Value){throw "Installed component mismatch: $($component.Key)"}}
 Record 'running-upgrade-and-installed-payload-hashes' $meta.components
 Checkpoint 'verify-resume';Record 'paused-task-resumes-after-upgrade-with-original-hash' $true
 Msi @('/x',$code,'/qn') 'upgrade-uninstall';$installed=$null
 if([CandidateAudit.MSI]::MsiQueryProductState($code) -ne -1 -or (Test-Path "$install\HLSDownloader.exe")){throw 'Upgrade uninstall incomplete'}
 Record 'upgrade-uninstall' $true
 Msi @('/i',"$work\candidate.msi",'/qn',"INSTALLDIR=$install") 'fresh-install';$installed=$code
 foreach($component in $expected.GetEnumerator()){if((Get-FileHash -LiteralPath (Join-Path $install $component.Key)).Hash.ToLowerInvariant() -ne $component.Value){throw "Fresh payload mismatch: $($component.Key)"}}
 Record 'fresh-install-exact-payload' $true
 $result.performance=@()
 foreach($cap in @(1,12)){
  try {Run C:\QAPython\python.exe @('C:\QACandidateRunners\candidate-transfer-probe.py','--engine',"$install\app\resources\HLSDownloaderEngine.exe",'--report',"$report\candidate-transfer-$cap.json",'--stage-root',"$work\performance",'--connections',"$cap") "candidate-transfer-$cap"} catch {
   if(-not(Test-Path "$report\candidate-transfer-$cap.json")){throw}
   $sample=Get-Content "$report\candidate-transfer-$cap.json" -Raw -Encoding UTF8 | ConvertFrom-Json
   if($sample.sha256 -ne '486cc817b95d853d3c357ff283b204c0144bd255e73fe2deb1389493b257e3c0' -or $sample.post_publish_extra_network_bytes -ne 0){throw}
  }
  $result.performance+=Get-Content "$report\candidate-transfer-$cap.json" -Raw -Encoding UTF8 | ConvertFrom-Json
 }
 $result.performance_threshold_passed=@($result.performance | Where-Object {-not $_.passed}).Count -eq 0
 Record 'optimized-installed-core-transfer-integrity-and-connection-cap-samples' $true
 # Use actual installed Core/Host/Presenter and archives for both browser matrices.
 $env:HLS_CURRENT_RESOURCES="$install\app\resources"
 foreach($item in @(@('Chromium','chrome-mv3'),@('Firefox','firefox-mv3'))){Expand-Archive -LiteralPath "$install\app\resources\extensions\HLSDownloader-7.0.3-$($item[0]).zip" -DestinationPath "C:\QACurrentExtensions\$($item[1])" -Force}
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
