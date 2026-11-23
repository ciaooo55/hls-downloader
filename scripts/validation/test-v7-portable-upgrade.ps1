[CmdletBinding()]
param(
    [string]$Shell = ''
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
Add-Type -AssemblyName System.IO.Compression.FileSystem

if ([String]::IsNullOrWhiteSpace($Shell)) {
    $Shell = if ($PSVersionTable.PSEdition -eq 'Desktop') { 'powershell.exe' } else { 'pwsh.exe' }
}

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$upgradeScriptSrc = Join-Path $repo 'scripts\upgrade-v7-portable.ps1'
$hashScriptSrc = Join-Path $repo 'scripts\V7HashFunctions.ps1'
$preflightSource = Join-Path $repo 'scripts\install\installer-preflight.rs'
$featureParitySource = Join-Path $repo 'config\release\feature-parity.json'
$targetVersion = [string](Get-Content -LiteralPath $featureParitySource -Raw -Encoding UTF8 | ConvertFrom-Json).product_version
if ([String]::IsNullOrWhiteSpace($targetVersion)) {
    throw "Unable to read product_version from $featureParitySource"
}
$baseVersion = if ($targetVersion -eq '10.0.0') { '7.0.10' } else { '10.0.0' }

$rustcCmd = Get-Command rustc -ErrorAction SilentlyContinue
$rustc = if ($rustcCmd) { $rustcCmd.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\rustc.exe' }
if (-not (Test-Path -LiteralPath $rustc)) {
    throw "rustc not found at $rustc"
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("hls-portable-test-" + [Guid]::NewGuid().ToString('N'))
$binDir = Join-Path $work 'bin'
New-Item -ItemType Directory -Path $binDir -Force | Out-Null

function New-ZipWithManifest([string]$ZipPath, [string]$ManifestJson) {
    $stage = Join-Path ([System.IO.Path]::GetDirectoryName($ZipPath)) ([Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    try {
        Set-Content -LiteralPath (Join-Path $stage 'manifest.json') -Value $ManifestJson -Encoding UTF8
        [System.IO.Compression.ZipFile]::CreateFromDirectory($stage, $ZipPath)
    } finally {
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function New-MockPortableImage([string]$Dir, [string]$Version) {
    $appRes = Join-Path $Dir 'app\resources'
    $extDir = Join-Path $Dir 'extensions'
    foreach ($d in @($appRes, $extDir, (Join-Path $Dir 'data'), (Join-Path $Dir 'downloads'))) {
        New-Item -ItemType Directory -Path $d -Force | Out-Null
    }
    Set-Content -LiteralPath (Join-Path $Dir 'HLSDownloader.exe') -Value $Version -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $Dir 'portable') -Value '1' -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $Dir 'VERSION') -Value $Version -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $appRes 'HLSDownloaderEngine.exe') -Value $Version -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $appRes 'HLSDownloaderPresenter.exe') -Value $Version -Encoding UTF8

    $featureParityPath = Join-Path $appRes 'FEATURE-PARITY.json'
    $featureParityJson = @{ product_version = $Version } | ConvertTo-Json -Compress
    Set-Content -LiteralPath $featureParityPath -Value $featureParityJson -Encoding UTF8
    $featureHash = (Get-FileHash -LiteralPath $featureParityPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $provenance = @{
        schema = 1
        product_version = $Version
        package_tier = 'candidate'
        ui_runtime = 'native'
        feature_parity_path = 'config/release/feature-parity.json'
        feature_parity_sha256 = $featureHash
        source_commit = 'a' * 40
        source_tree = 'b' * 40
    } | ConvertTo-Json -Depth 4
    Set-Content -LiteralPath (Join-Path $appRes 'BUILD-PROVENANCE.json') -Value $provenance -Encoding UTF8

    $chromiumManifest = @{
        manifest_version = 3
        version = $Version
        key = 'test-chromium-key'
    } | ConvertTo-Json -Compress
    $firefoxManifest = @{
        manifest_version = 3
        version = $Version
        browser_specific_settings = @{ gecko = @{ id = 'hls@test' } }
    } | ConvertTo-Json -Depth 4 -Compress
    New-ZipWithManifest (Join-Path $extDir "HLSDownloader-$Version-Chromium.zip") $chromiumManifest
    New-ZipWithManifest (Join-Path $extDir "HLSDownloader-$Version-Firefox.zip") $firefoxManifest
}

try {
    Copy-Item -LiteralPath $upgradeScriptSrc -Destination (Join-Path $binDir 'upgrade-v7-portable.ps1') -Force
    Copy-Item -LiteralPath $hashScriptSrc -Destination (Join-Path $binDir 'V7HashFunctions.ps1') -Force
    $upgradeScript = Join-Path $binDir 'upgrade-v7-portable.ps1'
    $preflightExe = Join-Path $binDir 'HLSDownloaderPreflight.exe'
    & $rustc --edition 2021 $preflightSource -o $preflightExe
    if ($LASTEXITCODE -ne 0) { throw 'Failed to compile installer-preflight.rs' }

    # Case 1: Clean upgrade ($baseVersion -> $targetVersion) preserves user data and downloads.
    $case1 = Join-Path $work 'case1'
    $target1 = Join-Path $case1 'target'
    $source1 = Join-Path $case1 'source'
    New-MockPortableImage $target1 $baseVersion
    New-MockPortableImage $source1 $targetVersion
    Set-Content -LiteralPath (Join-Path $target1 'data\state.txt') -Value 'user-data-1' -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $target1 'downloads\clip.mp4') -Value 'video-1' -Encoding UTF8

    & $Shell -NoProfile -ExecutionPolicy Bypass -File $upgradeScript -SourceDir $source1 -TargetDir $target1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Case 1 upgrade failed' }
    if ((Get-Content -LiteralPath (Join-Path $target1 'VERSION') -Raw).Trim() -ne $targetVersion) { throw "Case 1 target version was not upgraded to $targetVersion" }
    $upgradedProv = Get-Content -LiteralPath (Join-Path $target1 'app\resources\BUILD-PROVENANCE.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([string]$upgradedProv.product_version -ne $targetVersion) { throw "Case 1 provenance version was not upgraded to $targetVersion" }
    if (-not (Test-Path -LiteralPath (Join-Path $target1 "extensions\HLSDownloader-$targetVersion-Chromium.zip") -PathType Leaf)) { throw "Case 1 upgraded Chromium extension archive for $targetVersion is missing" }
    if ((Get-Content -LiteralPath (Join-Path $target1 'data\state.txt') -Raw).Trim() -ne 'user-data-1') { throw 'Case 1 user data lost after upgrade' }
    if ((Get-Content -LiteralPath (Join-Path $target1 'downloads\clip.mp4') -Raw).Trim() -ne 'video-1') { throw 'Case 1 downloads lost after upgrade' }

    # Case 2: Rollback cleanup lock must NOT undo a committed rollback back to half-deleted $current.
    Set-Content -LiteralPath (Join-Path $target1 'locked.tmp') -Value 'locked' -Encoding UTF8
    $rollbackWrapper = Join-Path $work 'run-rollback-locked.ps1'
    @"
`$ErrorActionPreference = 'Stop'
`$script:lockStream = `$null
function Remove-Item {
    param(
        [string]`$LiteralPath,
        [switch]`$Recurse,
        [switch]`$Force,
        [string]`$ErrorAction = 'Continue'
    )
    if (`$LiteralPath -like '*.v7-rollback-current' -and `$null -eq `$script:lockStream) {
        `$script:lockStream = [System.IO.File]::Open((Join-Path `$LiteralPath 'locked.tmp'), [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    }
    Microsoft.PowerShell.Management\Remove-Item -LiteralPath `$LiteralPath -Recurse:`$Recurse -Force:`$Force -ErrorAction `$ErrorAction
}
try {
    & '$upgradeScript' -Rollback -RollbackDir '$target1'
} finally {
    if (`$null -ne `$script:lockStream) { `$script:lockStream.Dispose() }
}
"@ | Set-Content -LiteralPath $rollbackWrapper -Encoding UTF8

    & $Shell -NoProfile -ExecutionPolicy Bypass -File $rollbackWrapper 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Case 2 rollback should succeed despite cleanup lock (exit $LASTEXITCODE)" }
    if ((Get-Content -LiteralPath (Join-Path $target1 'VERSION') -Raw).Trim() -ne $baseVersion) { throw "Case 2 committed rollback did not restore $baseVersion!" }
    $rolledBackProv = Get-Content -LiteralPath (Join-Path $target1 'app\resources\BUILD-PROVENANCE.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([string]$rolledBackProv.product_version -ne $baseVersion) { throw "Case 2 provenance version did not restore $baseVersion!" }
    if ((Get-Content -LiteralPath (Join-Path $target1 'data\state.txt') -Raw).Trim() -ne 'user-data-1') { throw 'Case 2 user data lost after rollback!' }

    # Case 3: Upgrade failure with locked $target must NOT nest $backup inside $target.
    $case3 = Join-Path $work 'case3'
    $target3 = Join-Path $case3 'target'
    $source3 = Join-Path $case3 'source'
    New-MockPortableImage $target3 $baseVersion
    New-MockPortableImage $source3 $targetVersion
    Set-Content -LiteralPath (Join-Path $target3 'data\state.txt') -Value 'user-data-3' -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $source3 'hold.tmp') -Value 'hold' -Encoding UTF8

    $upgradeWrapper = Join-Path $work 'run-upgrade-locked-fail.ps1'
    @"
`$ErrorActionPreference = 'Stop'
`$script:lockStream = `$null
function Copy-Item {
    param([Parameter(ValueFromRemainingArguments=`$true)]`$Args)
    `$script:lockStream = [System.IO.File]::Open((Join-Path '$target3' 'hold.tmp'), [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    throw 'simulated Copy-Preserved disk full'
}
try {
    & '$upgradeScript' -SourceDir '$source3' -TargetDir '$target3'
} finally {
    if (`$null -ne `$script:lockStream) { `$script:lockStream.Dispose() }
}
"@ | Set-Content -LiteralPath $upgradeWrapper -Encoding UTF8

    $failedAsExpected = $false
    try {
        & $Shell -NoProfile -ExecutionPolicy Bypass -File $upgradeWrapper 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { $failedAsExpected = $true }
    } catch {
        $failedAsExpected = $true
    }
    if (-not $failedAsExpected) { throw 'Case 3 simulated failure unexpectedly succeeded' }
    $backup3 = "$target3.v7-backup"
    if (-not (Test-Path -LiteralPath $backup3 -PathType Container)) { throw 'Case 3 intact backup disappeared!' }
    if (Test-Path -LiteralPath (Join-Path $target3 'target.v7-backup')) { throw 'Case 3 nested backup inside target!' }

    Write-Output ([ordered]@{
        schema = 1
        passed = $true
        shell = $Shell
        from_version = $baseVersion
        to_version = $targetVersion
        cases = @('clean-upgrade', 'rollback-cleanup-lock', 'upgrade-locked-target-no-nesting')
    } | ConvertTo-Json -Compress)
} finally {
    if (Test-Path -LiteralPath $work) {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}
