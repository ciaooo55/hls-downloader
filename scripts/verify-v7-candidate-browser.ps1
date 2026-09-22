[CmdletBinding()]
param(
    [string]$CandidateManifestPath = 'artifacts\v7-productization\candidate\ARTIFACT-MANIFEST.json',
    [Parameter(Mandatory=$true)][string]$EdgeBinary,
    [Parameter(Mandatory=$true)][string]$FirefoxBinary,
    [string]$Python = 'python.exe',
    [string]$EdgeDriver = '',
    [string]$FirefoxDriver = '',
    [string]$Ffmpeg = 'ffmpeg',
    [string]$Go = 'go'
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'V7HashFunctions.ps1')
$repo = (Resolve-Path "$PSScriptRoot\..").Path
$artifactRoot = Join-Path $repo 'artifacts\v7-productization\candidate-browser'
$manifestPath = if ([IO.Path]::IsPathRooted($CandidateManifestPath)) {
    [IO.Path]::GetFullPath($CandidateManifestPath)
} else {
    [IO.Path]::GetFullPath((Join-Path $repo $CandidateManifestPath))
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
$manifestRoot = Split-Path $manifestPath
$portable = [IO.Path]::GetFullPath((Join-Path $manifestRoot ([string]$manifest.artifacts.portable.path)))
if ((Get-FileHash $portable -Algorithm SHA256).Hash.ToLowerInvariant() -ne ([string]$manifest.artifacts.portable.sha256).ToLowerInvariant()) { throw 'Candidate Portable SHA-256 mismatch.' }
Remove-Item $artifactRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $artifactRoot | Out-Null
$env:TEMP = Join-Path $artifactRoot 'temp'; $env:TMP = $env:TEMP
New-Item -ItemType Directory -Force $env:TEMP | Out-Null
Expand-Archive $portable (Join-Path $artifactRoot 'portable') -Force
$root = Join-Path $artifactRoot 'portable\HLSDownloader'
$resources = Join-Path $root 'app\resources'
$extensions = Join-Path $artifactRoot 'extensions'; New-Item -ItemType Directory $extensions | Out-Null
foreach ($name in @('Chromium','Firefox')) {
    $entry = $manifest.extensions.$name
    $zip = Join-Path $root ([string]$entry.path)
    if ((Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant() -ne ([string]$entry.sha256).ToLowerInvariant()) { throw "Candidate $name extension SHA-256 mismatch." }
    $directory = if ($name -eq 'Chromium') { 'chrome-mv3' } else { 'firefox-mv3' }
    Expand-Archive $zip (Join-Path $extensions $directory) -Force
}
function Invoke-Smoke([string]$Script, [string[]]$Arguments) {
    & $Python (Join-Path $repo $Script) @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Script failed with exit $LASTEXITCODE" }
}
$edgeDriverArgs = if ($EdgeDriver) { @('--driver',$EdgeDriver) } else { @() }
$firefoxDriverArgs = if ($FirefoxDriver) { @('--driver',$FirefoxDriver) } else { @() }
$firefoxBrowserDriverArgs = if ($FirefoxDriver) { @('--firefox-driver',$FirefoxDriver) } else { @() }
$helperProcessNames = @('HLSDownloaderEngine', 'HLSDownloaderPresenter', 'HLSDownloaderNativeHost')
$initialHelperIds = @{}
foreach ($name in $helperProcessNames) {
    $initialHelperIds[$name] = @(Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
}
try {
    Invoke-Smoke 'scripts\smoke_v7_presenter.py' @('--presenter',(Join-Path $resources 'HLSDownloaderPresenter.exe'),'--host',(Join-Path $resources 'HLSDownloaderNativeHost.exe'),'--engine',(Join-Path $resources 'HLSDownloaderEngine.exe'),'--recovery-only')
    Invoke-Smoke 'scripts\smoke_extension_browsers.py' (@('--extension-output',$extensions,'--browser','both','--chrome-binary',$EdgeBinary,'--firefox-binary',$FirefoxBinary) + $firefoxBrowserDriverArgs)
    Invoke-Smoke 'scripts\smoke_extension_media.py' (@('--browser','edge','--extension',(Join-Path $extensions 'chrome-mv3'),'--browser-binary',$EdgeBinary,'--ffmpeg',$Ffmpeg) + $edgeDriverArgs)
    Invoke-Smoke 'scripts\smoke_extension_media.py' (@('--browser','firefox','--extension',(Join-Path $extensions 'firefox-mv3'),'--browser-binary',$FirefoxBinary,'--ffmpeg',$Ffmpeg) + $firefoxDriverArgs)
    Invoke-Smoke 'scripts\smoke_extension_takeover.py' (@('--extension',(Join-Path $extensions 'chrome-mv3'),'--browser','edge','--browser-binary',$EdgeBinary,'--go',$Go) + $edgeDriverArgs)
} finally {
    # The presenter smoke and the packaged app both spawn helpers. Leaving a
    # presenter behind poisons every later run: it holds the session-global
    # mutex Local\HLSDownloader.v7.presenter, so the next presenter exits with
    # code 0 before rendering. Stop every helper this run created, not just the
    # engine.
    foreach ($name in $helperProcessNames) {
        Get-Process -Name $name -ErrorAction SilentlyContinue |
            Where-Object { $_.Id -notin $initialHelperIds[$name] } |
            Stop-Process -Force -ErrorAction SilentlyContinue
    }
}
Write-Host '{"schema":1,"passed":true,"edge_chromium":true,"firefox":true,"media_recognition":true,"takeover_recovery":true,"presenter_pending_recovery":true}'
