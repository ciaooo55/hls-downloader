[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,
    [switch]$ReleaseReady
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$utf8 = [Text.UTF8Encoding]::new($false)

$pending = [ordered]@{}
Import-Module (Join-Path $PSScriptRoot 'V7VersionContract.psm1') -Force
[void](Assert-V7CandidateMsiVersion -ExpectedVersion $Version -MsiProductVersion $Version)

function Read-Utf8([string]$RelativePath) {
    if ($pending.Contains($RelativePath)) { return $pending[$RelativePath] }
    [IO.File]::ReadAllText((Join-Path $repo $RelativePath), [Text.Encoding]::UTF8).Replace("`r`n", "`n")
}

function Write-Utf8([string]$RelativePath, [string]$Content) {
    $pending[$RelativePath] = $Content
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
Replace-Required 'native_ui\Cargo.toml' '(?m)^version = "\d+\.\d+\.\d+"$' "version = `"$Version`""
Replace-Required 'crates\protocol\Cargo.toml' '(?m)^version = "\d+\.\d+\.\d+"$' "version = `"$Version`""
Replace-Required 'crates\client\Cargo.toml' '(?m)^version = "\d+\.\d+\.\d+"$' "version = `"$Version`""
Replace-Required 'native_clients\Cargo.toml' '(?m)^version = "\d+\.\d+\.\d+"$' "version = `"$Version`""
Replace-Required 'extension\package.json' '(?m)^  "version": "\d+\.\d+\.\d+",$' "  `"version`": `"$Version`"," 

Set-CargoLockLocalVersion 'Cargo.lock' 'hls-native-shell'
Set-CargoLockLocalVersion 'Cargo.lock' 'hls-native-ui'
Set-CargoLockLocalVersion 'Cargo.lock' 'hls-protocol'
Set-CargoLockLocalVersion 'Cargo.lock' 'hls-client'
Set-CargoLockLocalVersion 'Cargo.lock' 'hls-native-clients'

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
$readme = [regex]::Replace($readme, '(?m)^(\u5F53\u524D\u6E90\u7801\u76EE\u6807\u7248\u672C\u4E3A \[)\d+\.\d+\.\d+(\uFF08\u53D1\u5E03\u8349\u6848\uFF09\]\(docs/releases/v)\d+\.\d+\.\d+(\.md\))', "`${1}$Version`${2}$Version`${3}", 1)
Write-Utf8 $readmePath $readme

$readmeEnPath = 'README_EN.md'
if (Test-Path -LiteralPath (Join-Path $repo $readmeEnPath) -PathType Leaf) {
    $readmeEn = Read-Utf8 $readmeEnPath
    $readmeEn = [regex]::Replace($readmeEn, '(?m)^(>\s*`main` currently tracks the \*\*)\d+\.\d+\.\d+(\*\* source line\.)', "`${1}$Version`${2}", 1)
    $readmeEn = [regex]::Replace($readmeEn, '(?m)^(The current source targets \[)\d+\.\d+\.\d+( \(release draft\)\]\(docs/releases/v)\d+\.\d+\.\d+(\.md\))', "`${1}$Version`${2}$Version`${3}", 1)
    Write-Utf8 $readmeEnPath $readmeEn
}

foreach ($path in $pending.Keys) {
    [IO.File]::WriteAllText((Join-Path $repo $path), $pending[$path], $utf8)
}
Write-Host "Updated v7 product version and local Cargo lock entries to $Version (release_ready=$([bool]$ReleaseReady))."
