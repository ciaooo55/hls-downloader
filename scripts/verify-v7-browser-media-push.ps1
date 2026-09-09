[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$CandidateManifestPath,
    [string]$ExpectedTvboxHost = $env:HLS_V7_TVBOX_EXPECTED_HOST,
    [string]$InstallDir = 'E:\h',
    [string]$EdgeBinary = $env:HLS_V7_EDGE_BINARY,
    [string]$FirefoxBinary = $env:HLS_V7_FIREFOX_BINARY,
    [string]$EdgeDriver = $env:HLS_V7_EDGE_DRIVER,
    [string]$FirefoxDriver = $env:HLS_V7_FIREFOX_DRIVER,
    [string]$Python = $env:HLS_V7_PYTHON,
    [string]$Ffmpeg = ''
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$utf8NoBom = New-Object Text.UTF8Encoding($false)
$coreScript = Join-Path $PSScriptRoot 'verify-v7-browser-media-push-core.ps1'
if (-not (Test-Path -LiteralPath $coreScript -PathType Leaf)) {
    throw "Browser media-push core verifier is missing: $coreScript"
}

function Resolve-FullPath([string]$Path, [string]$Base) {
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $Base $Path))
}

$manifestPath = (Resolve-Path -LiteralPath $CandidateManifestPath).Path
$manifestRoot = Split-Path $manifestPath -Parent
$manifest = [IO.File]::ReadAllText($manifestPath, $utf8NoBom) | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
if ([int]$manifest.schema -ne 1 -or
    [string]$manifest.package_tier -ne 'candidate' -or
    [string]$manifest.source_commit -ne $currentCommit -or
    [string]$manifest.source_tree -ne $currentTree) {
    throw 'Browser media-push wrapper requires a candidate manifest from the current source commit/tree.'
}
$portableEntry = $manifest.artifacts.portable
if ($null -eq $portableEntry -or [String]::IsNullOrWhiteSpace([string]$portableEntry.path) -or [string]$portableEntry.sha256 -notmatch '^[0-9a-fA-F]{64}$') {
    throw 'Candidate manifest is missing a valid portable artifact entry for Native Host provenance.'
}
$portablePath = Resolve-FullPath ([string]$portableEntry.path) $manifestRoot
$manifestRootPrefix = [IO.Path]::GetFullPath($manifestRoot).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
if (-not $portablePath.StartsWith($manifestRootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Candidate portable artifact escaped the manifest root: $portablePath"
}
if (-not (Test-Path -LiteralPath $portablePath -PathType Leaf)) {
    throw "Candidate portable artifact is missing: $portablePath"
}
$portableSha256 = (Get-FileHash -LiteralPath $portablePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($portableSha256 -ne ([string]$portableEntry.sha256).ToLowerInvariant()) {
    throw "Candidate portable artifact SHA-256 mismatch: $portableSha256"
}

$tempRoot = Join-Path $env:RUNNER_TEMP ('hls-v7-native-host-provenance-' + [guid]::NewGuid().ToString('n'))
$packageResources = Join-Path $repo 'desktop_ui\resources\common'
try {
    if (Test-Path -LiteralPath $packageResources) {
        throw "Candidate build resource staging unexpectedly survived build cleanup: $packageResources"
    }
    New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
    Expand-Archive -LiteralPath $portablePath -DestinationPath $tempRoot -Force
    $portableNativeHost = Join-Path $tempRoot 'HLSDownloader\app\resources\HLSDownloaderNativeHost.exe'
    if (-not (Test-Path -LiteralPath $portableNativeHost -PathType Leaf)) {
        throw "Manifest-bound portable package is missing HLSDownloaderNativeHost.exe: $portableNativeHost"
    }
    $portableNativeHostSha256 = (Get-FileHash -LiteralPath $portableNativeHost -Algorithm SHA256).Hash.ToLowerInvariant()

    New-Item -ItemType Directory -Force -Path $packageResources | Out-Null
    $stagedNativeHost = Join-Path $packageResources 'HLSDownloaderNativeHost.exe'
    Copy-Item -LiteralPath $portableNativeHost -Destination $stagedNativeHost -Force
    $stagedNativeHostSha256 = (Get-FileHash -LiteralPath $stagedNativeHost -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($stagedNativeHostSha256 -ne $portableNativeHostSha256) {
        throw 'Temporary Native Host provenance staging changed the manifest-bound portable Host digest.'
    }

    $coreArguments = @{
        CandidateManifestPath = $manifestPath
        ExpectedTvboxHost = $ExpectedTvboxHost
        InstallDir = $InstallDir
    }
    if (-not [String]::IsNullOrWhiteSpace($EdgeBinary)) { $coreArguments.EdgeBinary = $EdgeBinary }
    if (-not [String]::IsNullOrWhiteSpace($FirefoxBinary)) { $coreArguments.FirefoxBinary = $FirefoxBinary }
    if (-not [String]::IsNullOrWhiteSpace($EdgeDriver)) { $coreArguments.EdgeDriver = $EdgeDriver }
    if (-not [String]::IsNullOrWhiteSpace($FirefoxDriver)) { $coreArguments.FirefoxDriver = $FirefoxDriver }
    if (-not [String]::IsNullOrWhiteSpace($Python)) { $coreArguments.Python = $Python }
    if (-not [String]::IsNullOrWhiteSpace($Ffmpeg)) { $coreArguments.Ffmpeg = $Ffmpeg }

    & $coreScript @coreArguments
} finally {
    Remove-Item -LiteralPath $packageResources -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
