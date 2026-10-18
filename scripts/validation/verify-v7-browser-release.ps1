[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$CandidateManifestPath,
    [string]$ExpectedTvboxHost = $env:HLS_V7_TVBOX_EXPECTED_HOST,
    [string]$EdgeBinary = $env:HLS_V7_EDGE_BINARY,
    [string]$FirefoxBinary = $env:HLS_V7_FIREFOX_BINARY,
    [string]$EdgeDriver = $env:HLS_V7_EDGE_DRIVER,
    [string]$FirefoxDriver = $env:HLS_V7_FIREFOX_DRIVER,
    [string]$Python = $env:HLS_V7_PYTHON,
    [string]$Ffmpeg = '',
    [string]$Go = ''
)

$ErrorActionPreference = 'Stop'
$pythonExe = (Get-Command $(if ($Python) { $Python } else { 'python.exe' }) -ErrorAction Stop).Source
$ffmpegExe = if ($Ffmpeg) { (Resolve-Path -LiteralPath $Ffmpeg).Path } elseif ($env:HLS_V7_FFMPEG_DIR) { Join-Path $env:HLS_V7_FFMPEG_DIR 'ffmpeg.exe' } else { (Get-Command ffmpeg.exe -ErrorAction Stop).Source }
$goExe = if ($Go) { (Resolve-Path -LiteralPath $Go).Path } else { (Get-Command go.exe -ErrorAction Stop).Source }

$portableArgs = @(
    '-CandidateManifestPath', $CandidateManifestPath,
    '-EdgeBinary', $EdgeBinary,
    '-FirefoxBinary', $FirefoxBinary,
    '-Python', $pythonExe,
    '-Ffmpeg', $ffmpegExe,
    '-Go', $goExe
)
if ($EdgeDriver) { $portableArgs += @('-EdgeDriver', $EdgeDriver) }
if ($FirefoxDriver) { $portableArgs += @('-FirefoxDriver', $FirefoxDriver) }
$portableOutput = @(& (Join-Path $PSScriptRoot 'verify-v7-candidate-browser.ps1') @portableArgs)
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$mediaPushArgs = @(
    '-CandidateManifestPath', $CandidateManifestPath,
    '-ExpectedTvboxHost', $ExpectedTvboxHost,
    '-EdgeBinary', $EdgeBinary,
    '-FirefoxBinary', $FirefoxBinary,
    '-Python', $pythonExe,
    '-Ffmpeg', $ffmpegExe
)
if ($EdgeDriver) { $mediaPushArgs += @('-EdgeDriver', $EdgeDriver) }
if ($FirefoxDriver) { $mediaPushArgs += @('-FirefoxDriver', $FirefoxDriver) }
$mediaPushOutput = @(& (Join-Path $PSScriptRoot 'verify-v7-browser-media-push.ps1') @mediaPushArgs)
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$portableOutput | Write-Output
$mediaPushOutput | Write-Output
Write-Output ([ordered]@{
    schema = 1
    passed = $true
    candidate_manifest = $CandidateManifestPath
    expected_tvbox_host = $ExpectedTvboxHost
    checks = @('portable_real_browsers', 'installed_native_host_tvbox')
} | ConvertTo-Json -Compress)
