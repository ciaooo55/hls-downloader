[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

function Read-Utf8([string]$RelativePath) {
    [IO.File]::ReadAllText((Join-Path $repo $RelativePath), [Text.Encoding]::UTF8)
}

function Match-One([string]$RelativePath, [string]$Pattern, [string]$Label) {
    $matches = [regex]::Matches((Read-Utf8 $RelativePath), $Pattern)
    if ($matches.Count -ne 1) {
        throw "${Label}: expected exactly one version match in $RelativePath, found $($matches.Count)."
    }
    return $matches[0].Groups[1].Value
}

function Assert-Version([string]$Expected, [string]$Actual, [string]$Label) {
    if ($Actual -ne $Expected) {
        throw "$Label version mismatch: expected $Expected, found $Actual. Run scripts\bump-v7-version.ps1 -Version $Expected to repair synchronized version contracts."
    }
}

$featurePath = 'artifacts\v7-productization\feature-parity.json'
$feature = Read-Utf8 $featurePath | ConvertFrom-Json
$version = [string]$feature.product_version
if ($version -notmatch '^\d+\.\d+\.\d+$') {
    throw "feature-parity product_version is not a semantic patch version: $version"
}

Assert-Version $version (Match-One 'native_shell\Cargo.toml' '(?m)^version = "(\d+\.\d+\.\d+)"$' 'Rust Core manifest') 'Rust Core manifest'
Assert-Version $version (Match-One 'presenter_ui\Cargo.toml' '(?m)^version = "(\d+\.\d+\.\d+)"$' 'Presenter manifest') 'Presenter manifest'
Assert-Version $version (Match-One 'extension\package.json' '(?m)^  "version": "(\d+\.\d+\.\d+)",$' 'Extension package') 'Extension package'
Assert-Version $version (Match-One 'desktop_ui\src\main\kotlin\com\hlsdownloader\desktop\Protocol.kt' '(?m)^    const val version = "(\d+\.\d+\.\d+)"$' 'Compose Product.version') 'Compose Product.version'
Assert-Version $version (Match-One 'desktop_ui\src\test\kotlin\com\hlsdownloader\desktop\ProtocolTest.kt' '(?m)^        assertEquals\("(\d+\.\d+\.\d+)", Product\.version\)$' 'Compose version contract test') 'Compose version contract test'

$gradle = Read-Utf8 'desktop_ui\build.gradle.kts'
foreach ($check in @(
    @{ Pattern = '(?m)^version = "(\d+\.\d+\.\d+)"$'; Label = 'Compose project version' },
    @{ Pattern = '(?m)^        packageVersion = "(\d+\.\d+\.\d+)"$'; Label = 'Compose packageVersion' },
    @{ Pattern = '(?m)^        description = "HLS Downloader (\d+\.\d+\.\d+)"$'; Label = 'Compose package description' }
)) {
    $matches = [regex]::Matches($gradle, $check.Pattern)
    if ($matches.Count -ne 1) { throw "$($check.Label): expected exactly one match, found $($matches.Count)." }
    Assert-Version $version $matches[0].Groups[1].Value $check.Label
}

function Assert-LockPackage([string]$RelativePath, [string]$PackageName) {
    $escaped = [regex]::Escape($PackageName)
    $pattern = "(?ms)\[\[package\]\]\r?\nname = `"$escaped`"\r?\nversion = `"(\d+\.\d+\.\d+)`""
    Assert-Version $version (Match-One $RelativePath $pattern "$RelativePath $PackageName") "$RelativePath $PackageName"
}

Assert-LockPackage 'native_shell\Cargo.lock' 'hls-native-shell'
Assert-LockPackage 'presenter_ui\Cargo.lock' 'hls-native-shell'
Assert-LockPackage 'presenter_ui\Cargo.lock' 'hls-native-ui'

$expectedAuditPrefix = 'v' + ($version -replace '\.', '_') + '_'
if (-not ([string]$feature.audit_state).StartsWith($expectedAuditPrefix, [StringComparison]::Ordinal)) {
    throw "feature-parity audit_state must be version-scoped with prefix $expectedAuditPrefix; found $($feature.audit_state)."
}

Write-Host "v7 version contract is synchronized at $version (release_ready=$([bool]$feature.release_ready), audit_state=$($feature.audit_state))."
