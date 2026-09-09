[CmdletBinding()]
param([string]$CandidateManifestPath = 'artifacts\v7-productization\candidate\ARTIFACT-MANIFEST.json')
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
$reportDir = Join-Path $repo 'artifacts\v7-productization\performance'
$runtime = Join-Path $reportDir 'candidate-runtime'
$manifestPath = [IO.Path]::GetFullPath((Join-Path $repo $CandidateManifestPath))
$manifest = Get-Content $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ([string]$manifest.package_tier -ne 'candidate' -or [string]::IsNullOrWhiteSpace([string]$manifest.product_version)) { throw 'Artifact manifest must identify a candidate product version.' }
$portable = [IO.Path]::GetFullPath((Join-Path (Split-Path $manifestPath) ([string]$manifest.artifacts.portable.path)))
if ((Get-FileHash $portable -Algorithm SHA256).Hash.ToLowerInvariant() -ne ([string]$manifest.artifacts.portable.sha256).ToLowerInvariant()) { throw 'Candidate Portable SHA-256 mismatch.' }
Remove-Item $runtime -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $runtime | Out-Null
$env:TEMP = Join-Path $runtime 'temp'; $env:TMP = $env:TEMP
New-Item -ItemType Directory -Force $env:TEMP | Out-Null
Expand-Archive $portable $runtime -Force
$candidate = Join-Path $runtime 'HLSDownloader'
$resources = Join-Path $candidate 'app\resources'
$launcherConfig = Get-ChildItem -LiteralPath $candidate -Filter 'HLSDownloader.cfg' -Recurse -File | Select-Object -First 1
if (-not $launcherConfig) { throw 'Candidate Compose launcher configuration is missing.' }
$launcherText = Get-Content -LiteralPath $launcherConfig.FullName -Raw -Encoding UTF8
$rendererMatch = [regex]::Match($launcherText, '(?m)-Dskiko\.renderApi=([A-Za-z0-9_]+)')
if (-not $rendererMatch.Success) { throw 'Candidate Compose launcher does not declare skiko.renderApi.' }
$composeRenderApi = $rendererMatch.Groups[1].Value.ToUpperInvariant()
if ($composeRenderApi -ne 'SOFTWARE') { throw "Candidate performance gate requires packaged SOFTWARE renderer, found $composeRenderApi." }
$python = if ($env:HLS_V7_PYTHON) { $env:HLS_V7_PYTHON } else { 'python.exe' }
function Invoke-Checked([string]$File, [string[]]$Arguments) { & $File @Arguments; if ($LASTEXITCODE -ne 0) { throw "$File failed with exit $LASTEXITCODE" } }
$frameReport = Join-Path $reportDir 'compose-1000-task-frames.json'
Invoke-Checked 'powershell.exe' @('-NoProfile','-ExecutionPolicy','Bypass','-File',(Join-Path $PSScriptRoot 'smoke-v7-compose-frames.ps1'),'-ReportPath',$frameReport,'-AppPath',(Join-Path $candidate 'HLSDownloader.exe'))
$frame = Get-Content $frameReport -Raw -Encoding UTF8 | ConvertFrom-Json
$hostReport = Join-Path $reportDir 'native-host-cold-start.json'
Invoke-Checked $python @((Join-Path $PSScriptRoot 'smoke_v7_native_host.py'),'--host',(Join-Path $resources 'HLSDownloaderNativeHost.exe'),'--engine',(Join-Path $resources 'HLSDownloaderEngine.exe'),'--report',$hostReport)
$nativeHostResult = Get-Content $hostReport -Raw -Encoding UTF8 | ConvertFrom-Json
$soakReport = Join-Path $reportDir 'candidate-runtime-soak.json'
Invoke-Checked $python @((Join-Path $PSScriptRoot 'soak_v7_runtime.py'),'--engine',(Join-Path $resources 'HLSDownloaderEngine.exe'),'--report',$soakReport,'--idle-seconds','30','--stress-requests','1000')
$soak = Get-Content $soakReport -Raw -Encoding UTF8 | ConvertFrom-Json
$transferReport = Join-Path $reportDir 'real-transfer-latest.json'
Invoke-Checked $python @((Join-Path $PSScriptRoot 'smoke_v7_transfer_performance.py'),'--engine',(Join-Path $resources 'HLSDownloaderEngine.exe'),'--report',$transferReport)
$transfer = Get-Content $transferReport -Raw -Encoding UTF8 | ConvertFrom-Json
$result = [ordered]@{
 schema=1; product_version=[string]$manifest.product_version; candidate_manifest=$manifestPath; candidate_source_commit=[string]$manifest.source_commit; candidate_source_tree=[string]$manifest.source_tree; compose_render_api=$composeRenderApi; measured_at=[DateTime]::UtcNow.ToString('o')
 thousand_task_frame_p95_ms=$frame.frame_p95_ms; ipc_command_p95_ms=$soak.stress.ipc_p95_ms; native_host_cold_start_ms=$nativeHostResult.cold_first_response_ms
 real_transfer_throughput_mib_s=$transfer.throughput_mib_s; real_transfer_working_set_growth_mib=$transfer.working_set_growth_mib; post_publish_extra_network_bytes=$transfer.post_publish_extra_network_bytes
 thresholds=[ordered]@{ thousand_task_frame_p95_ms=33; ipc_command_p95_ms=75; native_host_cold_start_ms=1500; minimum_local_throughput_mib_s=20; maximum_working_set_growth_mib=256; post_publish_extra_network_bytes=0 }
 passed=($frame.passed -and [double]$soak.stress.ipc_p95_ms -le 75 -and $nativeHostResult.cold_first_response_ms -le 1500 -and $transfer.passed)
}
$reportPath = Join-Path $reportDir 'v7-performance-latest.json'
[IO.File]::WriteAllText($reportPath, ($result | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
if (-not $result.passed) { throw "v7 candidate performance threshold failed: $($result | ConvertTo-Json -Compress)" }
Write-Host ($result | ConvertTo-Json -Compress)
