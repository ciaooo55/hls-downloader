[CmdletBinding()]
param(
    [string]$CandidateManifestPath = 'artifacts\v7-productization\candidate\ARTIFACT-MANIFEST.json',
    [string]$EdgeBinary = $env:HLS_V7_EDGE_BINARY,
    [string]$FirefoxBinary = $env:HLS_V7_FIREFOX_BINARY,
    [string]$EdgeDriver = $env:HLS_V7_EDGE_DRIVER,
    [string]$FirefoxDriver = $env:HLS_V7_FIREFOX_DRIVER
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$manifestFullPath = if ([IO.Path]::IsPathRooted($CandidateManifestPath)) { [IO.Path]::GetFullPath($CandidateManifestPath) } else { [IO.Path]::GetFullPath((Join-Path $repo $CandidateManifestPath)) }
if (-not (Test-Path -LiteralPath $manifestFullPath -PathType Leaf)) { throw "Candidate manifest is missing: $manifestFullPath" }
$manifest = Get-Content -LiteralPath $manifestFullPath -Raw -Encoding UTF8 | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
if ([int]$manifest.schema -ne 1 -or [string]$manifest.package_tier -ne 'candidate' -or [string]$manifest.source_commit -ne $currentCommit -or [string]$manifest.source_tree -ne $currentTree) {
    throw 'Release gates require a candidate from the current source commit/tree.'
}
if (-not (Test-Path -LiteralPath 'E:\' -PathType Container)) { throw 'The formal release runner must provide the E: volume used by the MSI lifecycle gate.' }

function Resolve-Browser([string]$Explicit, [string[]]$Defaults, [string]$Label) {
    if (-not [String]::IsNullOrWhiteSpace($Explicit)) {
        $resolved = (Resolve-Path -LiteralPath $Explicit).Path
        if (Test-Path -LiteralPath $resolved -PathType Leaf) { return $resolved }
    }
    foreach ($candidate in $Defaults) {
        if (-not [String]::IsNullOrWhiteSpace($candidate) -and (Test-Path -LiteralPath $candidate -PathType Leaf)) { return (Resolve-Path -LiteralPath $candidate).Path }
    }
    throw "$Label browser binary was not found. Set the corresponding HLS_V7_*_BINARY variable on the release runner."
}
function Quote-PS([string]$Value) { return "'" + $Value.Replace("'", "''") + "'" }
function Add-OptionalArgument([Collections.Generic.List[string]]$Parts, [string]$Name, [string]$Value) {
    if (-not [String]::IsNullOrWhiteSpace($Value)) {
        $resolved = (Resolve-Path -LiteralPath $Value).Path
        $Parts.Add($Name)
        $Parts.Add((Quote-PS $resolved))
    }
}
function Invoke-Gate([string]$Id, [string]$Command, [string]$InputDescription) {
    & powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'record-v7-release-gate.ps1') -GateId $Id -Command $Command -GateInput $InputDescription -CandidateManifestPath $manifestFullPath
    $gateExitCode = $LASTEXITCODE
    if ($gateExitCode -ne 0) { throw "Release gate $Id failed with exit code $gateExitCode." }
}

$edge = Resolve-Browser $EdgeBinary @(
    (Join-Path ${env:ProgramFiles(x86)} 'Microsoft\Edge\Application\msedge.exe'),
    (Join-Path $env:ProgramFiles 'Microsoft\Edge\Application\msedge.exe')
) 'Edge'
$firefox = Resolve-Browser $FirefoxBinary @(
    (Join-Path $env:ProgramFiles 'Mozilla Firefox\firefox.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Mozilla Firefox\firefox.exe')
) 'Firefox'
$ffmpeg = if ($env:HLS_V7_FFMPEG_DIR) { Join-Path $env:HLS_V7_FFMPEG_DIR 'ffmpeg.exe' } else { (Get-Command ffmpeg.exe -ErrorAction Stop).Source }
if (-not (Test-Path -LiteralPath $ffmpeg -PathType Leaf)) { throw "FFmpeg is missing: $ffmpeg" }
$go = (Get-Command go.exe -ErrorAction Stop).Source
$python = (Get-Command $(if ($env:HLS_V7_PYTHON) { $env:HLS_V7_PYTHON } else { 'python.exe' }) -ErrorAction Stop).Source
$env:HLS_V7_PYTHON = $python

$evidenceRoot = Join-Path $repo 'artifacts\v7-productization\release-evidence'
Remove-Item -LiteralPath $evidenceRoot -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath (Join-Path $repo 'artifacts\v7-productization\release-evidence.json') -Force -ErrorAction SilentlyContinue

$browserParts = [Collections.Generic.List[string]]::new()
$browserParts.Add('&')
$browserParts.Add((Quote-PS (Join-Path $PSScriptRoot 'verify-v7-candidate-browser.ps1')))
$browserParts.Add('-CandidateManifestPath'); $browserParts.Add((Quote-PS $manifestFullPath))
$browserParts.Add('-EdgeBinary'); $browserParts.Add((Quote-PS $edge))
$browserParts.Add('-FirefoxBinary'); $browserParts.Add((Quote-PS $firefox))
$browserParts.Add('-Python'); $browserParts.Add((Quote-PS $python))
$browserParts.Add('-Ffmpeg'); $browserParts.Add((Quote-PS $ffmpeg))
$browserParts.Add('-Go'); $browserParts.Add((Quote-PS $go))
Add-OptionalArgument $browserParts '-EdgeDriver' $EdgeDriver
Add-OptionalArgument $browserParts '-FirefoxDriver' $FirefoxDriver
Invoke-Gate 'browser' ($browserParts -join ' ') "candidate portable; Edge=$edge; Firefox=$firefox"

$performanceCommand = "& $(Quote-PS (Join-Path $PSScriptRoot 'benchmark-v7.ps1')) -CandidateManifestPath $(Quote-PS $manifestFullPath)"
Invoke-Gate 'performance' $performanceCommand 'candidate Portable; packaged SOFTWARE renderer; local transfer/IPC/host/frame thresholds'

$upgradeCommand = "& $(Quote-PS (Join-Path $PSScriptRoot 'verify-v7-msi-lifecycle.ps1')) -Scenario Upgrade -CandidateManifestPath $(Quote-PS $manifestFullPath) -InstallDir 'E:\h'"
Invoke-Gate 'installer' $upgradeCommand 'public v7.0.0 MSI -> candidate v7.0.1 MSI at E:\h; checkpoint/process recovery'

$rollbackCommand = "& $(Quote-PS (Join-Path $PSScriptRoot 'verify-v7-msi-lifecycle.ps1')) -Scenario FailureRollback -CandidateManifestPath $(Quote-PS $manifestFullPath) -InstallDir 'E:\h'"
Invoke-Gate 'rollback' $rollbackCommand 'candidate MSI Type-19 failure injection at E:\h; v7.0.0 product/data/registration preserved'

$aggregate = Join-Path $repo 'artifacts\v7-productization\release-evidence.json'
$evidence = Get-Content -LiteralPath $aggregate -Raw -Encoding UTF8 | ConvertFrom-Json
$passed = @($evidence.gates | Where-Object { $_.result -eq 'passed' -and [int]$_.exit_status -eq 0 })
if (@($evidence.gates).Count -ne 4 -or $passed.Count -ne 4) { throw 'All four release gates were not recorded as passed.' }
Write-Output ([ordered]@{ schema = 1; passed = $true; product_version = [string]$manifest.product_version; source_commit = $currentCommit; gates = @($evidence.gates | ForEach-Object { $_.id }) } | ConvertTo-Json -Compress)
