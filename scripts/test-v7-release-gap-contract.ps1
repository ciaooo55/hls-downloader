[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Shell
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$assertScript = Join-Path $repo 'scripts\assert-v7-release-gaps.ps1'
$shellPath = (Get-Command $Shell -ErrorAction Stop).Source
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('hls-v7-gap-contract-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

function Write-Fixture([string]$Path, [bool]$WithGap) {
    $feature = [ordered]@{
        schema = 1
        product_version = '7.0.2'
        release_ready = $false
        audit_state = 'test'
        features = @(
            [ordered]@{
                id = 'test.feature'
                status = $(if ($WithGap) { 'verified' } else { 'partial' })
                verification = 'fixture'
            }
        )
    }
    if ($WithGap) {
        $feature.features[0].gap = 'candidate-bound real-device evidence is missing'
    }
    [IO.File]::WriteAllText($Path, ($feature | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
}

function Invoke-Assert([string]$Fixture) {
    $output = @(& $shellPath -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $assertScript -FeatureParityPath $Fixture 2>&1)
    return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = ($output -join "`n") }
}

try {
    $clean = Join-Path $tempRoot 'clean.json'
    $gapped = Join-Path $tempRoot 'gapped.json'
    Write-Fixture $clean $false
    Write-Fixture $gapped $true

    $cleanResult = Invoke-Assert $clean
    if ($cleanResult.ExitCode -ne 0 -or $cleanResult.Output -notmatch '"passed":true') {
        throw "No-gap fixture must pass under $Shell. exit=$($cleanResult.ExitCode) output=$($cleanResult.Output)"
    }

    $gapResult = Invoke-Assert $gapped
    if ($gapResult.ExitCode -eq 0) {
        throw "Residual-gap fixture must fail under $Shell. output=$($gapResult.Output)"
    }
    if ($gapResult.Output -notmatch 'Formal release is blocked by residual canonical feature gap') {
        throw "Residual-gap failure under $Shell did not expose the fail-closed reason: $($gapResult.Output)"
    }

    Write-Output ([ordered]@{
        schema = 1
        passed = $true
        shell = $Shell
        clean_exit = $cleanResult.ExitCode
        gap_exit = $gapResult.ExitCode
    } | ConvertTo-Json -Compress)
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
