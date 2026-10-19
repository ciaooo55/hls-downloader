[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,
    [switch]$ReleaseReady
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$utf8 = [Text.UTF8Encoding]::new($false)

function Read-Utf8([string]$RelativePath) {
    [IO.File]::ReadAllText((Join-Path $repo $RelativePath), [Text.Encoding]::UTF8).Replace("`r`n", "`n")
}

function Write-Utf8([string]$RelativePath, [string]$Content) {
    [IO.File]::WriteAllText((Join-Path $repo $RelativePath), $Content, $utf8)
}

function Replace-Required([string]$RelativePath, [string]$Pattern, [string]$Replacement, [int]$ExpectedCount = 1) {
    $content = Read-Utf8 $RelativePath
    $matches = [regex]::Matches($content, $Pattern)
    if ($matches.Count -ne $ExpectedCount) {
        throw "$RelativePath expected $ExpectedCount version match(es), found $($matches.Count): $Pattern"
    }
    Write-Utf8 $RelativePath ([regex]::Replace($content, $Pattern, $Replacement))
}

function Set-CargoLockLocalVersion([string]$RelativePath, [string]$PackageName) {
    $content = Read-Utf8 $RelativePath
    $escaped = [regex]::Escape($PackageName)
    $pattern = "(?ms)(\[\[package\]\]\r?\nname = `"$escaped`"\r?\nversion = `")\d+\.\d+\.\d+(`")"
    $matches = [regex]::Matches($content, $pattern)
    if ($matches.Count -ne 1) {
        throw "$RelativePath expected exactly one local package entry for $PackageName, found $($matches.Count)."
    }
    Write-Utf8 $RelativePath ([regex]::Replace($content, $pattern, ('${1}' + $Version + '${2}'), 1))
}

Replace-Required 'native_shell\Cargo.toml' '(?m)^version = "\d+\.\d+\.\d+"$' "version = `"$Version`""
Replace-Required 'presenter_ui\Cargo.toml' '(?m)^version = "\d+\.\d+\.\d+"$' "version = `"$Version`""
Replace-Required 'extension\package.json' '(?m)^  "version": "\d+\.\d+\.\d+",$' "  `"version`": `"$Version`"," 
Replace-Required 'desktop_ui\src\main\kotlin\com\hlsdownloader\desktop\Protocol.kt' '(?m)^    const val version = "\d+\.\d+\.\d+"$' "    const val version = `"$Version`""
Replace-Required 'desktop_ui\build.gradle.kts' '(?m)^version = "\d+\.\d+\.\d+"$' "version = `"$Version`""
Replace-Required 'desktop_ui\build.gradle.kts' '(?m)^        packageVersion = "\d+\.\d+\.\d+"$' "        packageVersion = `"$Version`""
Replace-Required 'desktop_ui\build.gradle.kts' '(?m)^        description = "HLS Downloader \d+\.\d+\.\d+"$' "        description = `"HLS Downloader $Version`""

Set-CargoLockLocalVersion 'native_shell\Cargo.lock' 'hls-native-shell'
Set-CargoLockLocalVersion 'presenter_ui\Cargo.lock' 'hls-native-shell'
Set-CargoLockLocalVersion 'presenter_ui\Cargo.lock' 'hls-native-ui'

$featurePath = 'config\release\feature-parity.json'
$feature = Read-Utf8 $featurePath | ConvertFrom-Json
$feature.product_version = $Version
$feature.release_ready = [bool]$ReleaseReady
$feature.audit_state = if ($ReleaseReady) {
    'v' + ($Version -replace '\.', '_') + '_release_ready'
} else {
    'v' + ($Version -replace '\.', '_') + '_iteration_in_progress'
}
Write-Utf8 $featurePath (($feature | ConvertTo-Json -Depth 20) + "`n")

$readmePath = 'README.md'
$readme = Read-Utf8 $readmePath
$readme = [regex]::Replace($readme, '(?m)^(>\s*\u5F53\u524D `main` \u662F \*\*)\d+\.\d+\.\d+(\*\* \u6E90\u7801\u5F00\u53D1\u7EBF\u3002)', "`${1}$Version`${2}", 1)
Write-Utf8 $readmePath $readme

$readmeEnPath = 'README_EN.md'
if (Test-Path -LiteralPath (Join-Path $repo $readmeEnPath) -PathType Leaf) {
    $readmeEn = Read-Utf8 $readmeEnPath
    $readmeEn = [regex]::Replace($readmeEn, '(?m)^(>\s*`main` currently tracks the \*\*)\d+\.\d+\.\d+(\*\* source line\.)', "`${1}$Version`${2}", 1)
    Write-Utf8 $readmeEnPath $readmeEn
}

Write-Host "Updated v7 product version and local Cargo lock entries to $Version (release_ready=$([bool]$ReleaseReady))."
