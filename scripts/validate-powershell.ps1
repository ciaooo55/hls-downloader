param(
    [string[]]$Path = @()
)

$ErrorActionPreference = "Stop"
$usingDefaultPath = $Path.Count -eq 0
if ($usingDefaultPath) {
    # New release-gate scripts must enter syntax validation automatically. A
    # hard-coded list previously let media-push wrapper/core scripts bypass the
    # PowerShell 5.1/7 parser job entirely.
    $Path = @(
        Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.ps1' -File |
            Sort-Object Name |
            Select-Object -ExpandProperty FullName
    )
}

$failed = $false
foreach ($item in $Path) {
    $candidate = if ([IO.Path]::IsPathRooted($item)) { $item } else { Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..')).Path $item }
    $resolved = (Resolve-Path -LiteralPath $candidate).Path
    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile(
        $resolved,
        [ref]$tokens,
        [ref]$errors
    ) | Out-Null
    foreach ($parseError in @($errors)) {
        $failed = $true
        Write-Error "$resolved`: $($parseError.Message)"
    }
}
if ($failed) {
    exit 1
}
Write-Output "PowerShell syntax validation passed for $($Path.Count) scripts."

if ($usingDefaultPath) {
    # Keep the formal release caller and recorder in one executable contract.
    # C018 added browser_media_push as a fifth Invoke-Gate call but the recorder
    # retained a four-value ValidateSet; syntax-only checks could not detect it.
    # Run this before verify-v7-version-contract.ps1 because that legacy script
    # intentionally exits its PowerShell host after completing its checks.
    $invokePath = Join-Path $PSScriptRoot 'invoke-v7-release-gates.ps1'
    $recorderPath = Join-Path $PSScriptRoot 'record-v7-release-gate.ps1'
    $invokeSource = Get-Content -LiteralPath $invokePath -Raw -Encoding UTF8
    $recorderSource = Get-Content -LiteralPath $recorderPath -Raw -Encoding UTF8

    $invokedGateIds = @(
        [regex]::Matches($invokeSource, "Invoke-Gate\s+'([^']+)'") |
            ForEach-Object { $_.Groups[1].Value } |
            Sort-Object -Unique
    )
    if ($invokedGateIds.Count -ne 5) {
        throw "Formal release must invoke exactly five unique gates; found $($invokedGateIds.Count): $($invokedGateIds -join ', ')"
    }

    $gateParameter = [regex]::Match(
        $recorderSource,
        '(?s)\[ValidateSet\((.*?)\)\]\s*\[string\]\$GateId'
    )
    if (-not $gateParameter.Success) {
        throw 'Could not resolve the GateId ValidateSet from record-v7-release-gate.ps1.'
    }
    $acceptedGateIds = @(
        [regex]::Matches($gateParameter.Groups[1].Value, "'([^']+)'") |
            ForEach-Object { $_.Groups[1].Value } |
            Sort-Object -Unique
    )
    $missingGateIds = @($invokedGateIds | Where-Object { $_ -notin $acceptedGateIds })
    if ($missingGateIds.Count -ne 0) {
        throw "Formal release invokes GateId values rejected by the recorder: $($missingGateIds -join ', ')"
    }
    Write-Output "Formal release gate-id contract passed: $($invokedGateIds -join ', ')."

    # The default validation path also acts as an early product-version drift
    # gate before expensive Rust/Compose/Candidate work starts. Keep this last:
    # verify-v7-version-contract.ps1 exits the current PowerShell process.
    & (Join-Path $PSScriptRoot 'verify-v7-version-contract.ps1')
}
