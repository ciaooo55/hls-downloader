param([ValidateSet('software','gpu')][string]$Mode,[switch]$BrowserOnly)
$ErrorActionPreference='Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Sandbox-only runner' }
$env:HLS_VM_ACCEPTANCE='1'
$report='C:\QAReports'
$inputRoot='C:\QAInput'
New-Item -ItemType Directory -Force -Path C:\QAWork,$report | Out-Null
$steps=New-Object Collections.ArrayList
$summary=[ordered]@{ mode=$Mode; browser_only=[bool]$BrowserOnly; passed=$false; steps=$steps; error=''; finished=$false }
function Save-Status { $summary | ConvertTo-Json -Depth 8 | Set-Content "$report\summary.json" -Encoding UTF8 }
function Invoke-Check([string]$Name,[string]$Exe,[string[]]$Arguments,[int]$Timeout=180) {
  $summary.current=$Name; Save-Status
  $processArguments=@($Arguments | ForEach-Object { if ($_ -match '\s') { '"'+$_+'"' } else { $_ } })
  $process=Start-Process -FilePath $Exe -ArgumentList $processArguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "$report\$Name.stdout.log" -RedirectStandardError "$report\$Name.stderr.log"
  [void]$process.Handle
  if (-not $process.WaitForExit($Timeout*1000)) { & taskkill.exe /PID $process.Id /T /F | Out-Null; throw "$Name timed out" }
  [void]$steps.Add([ordered]@{name=$Name;exit_code=$process.ExitCode;passed=($process.ExitCode -eq 0)})
  Save-Status
  if ($process.ExitCode -ne 0) { throw "$Name exited $($process.ExitCode)" }
}
try {
  Save-Status
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  [IO.Compression.ZipFile]::ExtractToDirectory("$inputRoot\python-bundle.zip",'C:\QAPython')
  Add-Content 'C:\QAPython\python312._pth' 'C:\QAWork\source\scripts\validation'
  [IO.Compression.ZipFile]::ExtractToDirectory("$inputRoot\runtime-bundle.zip",'C:\QAWork\baseline')
  Copy-Item "$inputRoot\source" C:\QAWork\source -Recurse
  Copy-Item "$inputRoot\extension" C:\QAWork\extension -Recurse
  $python='C:\QAPython\python.exe'
  $scripts='C:\QAWork\source\scripts\validation'
  $ffmpeg='C:\QAWork\baseline\app\resources\ffmpeg.exe'
  if (-not $BrowserOnly) {
  Invoke-Check 'generate-media' $ffmpeg @('-nostdin','-y','-v','error','-f','lavfi','-i','testsrc2=size=640x360:rate=25','-t','20','-c:v','libx264','-preset','ultrafast','-pix_fmt','yuv420p','C:\QAWork\media.mp4')
  @{ wsb_vgpu=$Mode; source='Windows Sandbox configuration' } | ConvertTo-Json | Set-Content "$report\gpu.json" -Encoding UTF8
  Invoke-Check 'baseline-rendering' $python @("$scripts\smoke_v7_player_rendering.py",'--engine','C:\QAWork\baseline\app\resources\HLSDownloaderEngine.exe','--libmpv','C:\QAWork\baseline\app\resources\libmpv-2.dll','--media','C:\QAWork\media.mp4','--environment',$Mode,'--report',"$report\baseline-rendering.json") 100
  }
  $deadline=(Get-Date).AddMinutes(10)
  while (-not (Test-Path "$inputRoot\ready")) { if ((Get-Date) -gt $deadline) { throw 'Candidate build not ready' }; Start-Sleep -Seconds 2 }
  Copy-Item C:\QAWork\baseline C:\QAWork\runtime -Recurse
  Copy-Item "$inputRoot\candidate-engine.exe" C:\QAWork\runtime\app\resources\HLSDownloaderEngine.exe -Force
  $engine='C:\QAWork\runtime\app\resources\HLSDownloaderEngine.exe'
  if (-not $BrowserOnly) {
  Invoke-Check 'candidate-rendering' $python @("$scripts\smoke_v7_player_rendering.py",'--engine',$engine,'--libmpv','C:\QAWork\runtime\app\resources\libmpv-2.dll','--media','C:\QAWork\media.mp4','--environment',$Mode,'--expected-hwdec','auto-safe','--report',"$report\candidate-rendering.json") 100
  Invoke-Check 'gpu-renderer-comparison' $python @("$scripts\smoke_v7_player_rendering.py",'--engine',$engine,'--libmpv','C:\QAWork\runtime\app\resources\libmpv-2.dll','--media','C:\QAWork\media.mp4','--environment',$Mode,'--vo','gpu','--expected-hwdec','auto-safe','--report',"$report\gpu-renderer-comparison.json") 100
  Invoke-Check 'download-playback-cleanup' $python @("$scripts\smoke_v7_download_playback.py",'--runtime','C:\QAWork\runtime','--report',"$report\download-playback-cleanup.json") 300
  }
  if ($Mode -eq 'gpu') {
    if (-not $BrowserOnly) {
    Copy-Item "$inputRoot\core-tests.exe" C:\QAWork\runtime\app\resources\core-tests.exe
    Invoke-Check 'core-player-tests' 'C:\QAWork\runtime\app\resources\core-tests.exe' @('player::tests','--test-threads=1') 120
    Invoke-Check 'core-http-tests' 'C:\QAWork\runtime\app\resources\core-tests.exe' @('http_engine::tests','--test-threads=1') 180
    Invoke-Check 'http-boundaries' $python @("$scripts\smoke_v7_http_boundaries.py",'--engine',$engine,'--report',"$report\http-boundaries.json") 90
    }
    $edge='C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
    $edgeVersion=(Get-Item $edge).VersionInfo.FileVersion
    $edgeVersion | Set-Content "$report\edge-version.txt"
    Invoke-WebRequest "https://msedgedriver.microsoft.com/$edgeVersion/edgedriver_win64.zip" -OutFile C:\QAWork\edgedriver.zip -UseBasicParsing
    [IO.Compression.ZipFile]::ExtractToDirectory('C:\QAWork\edgedriver.zip','C:\QAWork\driver')
    $env:HLS_V7_DATA_DIR='C:\QAWork\browser-data'
    $env:HLS_V7_DOWNLOAD_DIR='C:\QAWork\downloads'
    Invoke-Check 'register-host' $engine @('--register-native-host')
    $core=Start-Process $engine -WindowStyle Hidden -PassThru -RedirectStandardOutput "$report\core.stdout.log" -RedirectStandardError "$report\core.stderr.log"
    Invoke-Check 'configure-core' $python @("$inputRoot\configure-core.py") 40
    Invoke-Check 'takeover' $python @("$scripts\smoke_extension_takeover.py",'--browser','edge','--extension','C:\QAWork\extension','--browser-binary',$edge,'--driver','C:\QAWork\driver\msedgedriver.exe','--real-runtime','C:\QAWork\runtime','--report',"$report\takeover.json") 420
    Invoke-Check 'public-takeover' $python @("$scripts\smoke_extension_public_takeover.py",'--browser','edge','--extension','C:\QAWork\extension','--runtime','C:\QAWork\runtime','--driver','C:\QAWork\driver\msedgedriver.exe','--report',"$report\public-takeover.json") 180
    Invoke-Check 'unregister-host' $engine @('--unregister-native-host')
    & taskkill.exe /PID $core.Id /T /F | Out-Null
    Invoke-Check 'installer' $python @("$scripts\smoke_v7_installed_footprint.py",'--msi',"$inputRoot\candidate.msi",'--manifest',"$inputRoot\candidate-manifest.json",'--expected-components',"$inputRoot\expected-components.json",'--report',"$report\installer.json") 360
  }
  $summary.passed=$true
} catch { $summary.error=$_.ToString(); $_ | Out-String | Set-Content "$report\runner-error.txt" }
finally { $summary.finished=$true; Save-Status }
