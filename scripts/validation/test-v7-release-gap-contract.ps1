[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Shell
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$assertScript = Join-Path $repo 'scripts\release\assert-v7-release-gaps.ps1'
$shellPath = (Get-Command $Shell -ErrorAction Stop).Source
$tempRoot = Join-Path (Join-Path $repo '.tool-cache\test-tmp') ('hls-v7-gap-contract-' + [guid]::NewGuid().ToString('n'))
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

function Write-Empty-Fixture([string]$Path) {
    # A matrix that lists no feature at all must never look like "no gaps": the gap
    # filter trivially matches nothing, so without a guard the assert script would
    # emit passed=true for a matrix that verifies nothing at all.
    $feature = [ordered]@{
        schema = 1
        product_version = '7.0.2'
        release_ready = $false
        audit_state = 'test'
        features = @()
    }
    [IO.File]::WriteAllText($Path, ($feature | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
}

function Invoke-Assert([string]$Fixture) {
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        # The gapped fixture is expected to make the child shell exit non-zero.
        # Windows PowerShell surfaces child stderr as ErrorRecord objects, so
        # keep those records non-terminating here and assert the exit code below.
        $ErrorActionPreference = 'Continue'
        $output = @(& $shellPath -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $assertScript -FeatureParityPath $Fixture 2>&1 |
            ForEach-Object { $_.ToString() })
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    return [pscustomobject]@{ ExitCode = $exitCode; Output = ($output -join "`n") }
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
    $empty = Join-Path $tempRoot 'empty.json'
    Write-Empty-Fixture $empty
    $emptyResult = Invoke-Assert $empty
    if ($emptyResult.ExitCode -eq 0) {
        throw "Empty-feature fixture must fail under $Shell. output=$($emptyResult.Output)"
    }
    if ($emptyResult.Output -notmatch 'zero features') {
        throw "Empty-feature failure under $Shell did not expose the fail-closed reason: $($emptyResult.Output)"
    }

    $workflow = Get-Content (Join-Path $repo '.github\workflows\release-v7.yml') -Raw -Encoding UTF8
    $fetch = @([regex]::Matches($workflow, '(?m)^\s+git fetch[^\r\n]+'))
    if ($fetch.Count -ne 1) { throw 'Expected one release tag fetch command.' }
    $fetchCommand = [scriptblock]::Create($fetch[0].Value.Trim())
    foreach ($tag in @('v7.0.4', 'v7.0.5-rc1')) {
        $arguments = @(& {
            function git { $args }
            & $fetchCommand
        })
        if ($arguments[-1] -ne ('refs/tags/{0}:refs/tags/{0}' -f $tag)) {
            throw "Release tag interpolation produced an invalid fetch refspec: $($arguments[-1])"
        }
    }

    # Exercise the production filter: PowerShell gives -and and -or the same
    # precedence, so an ungrouped exclusion clause also rejects passed gates.
    $gateScript = Join-Path $repo 'scripts\release\invoke-v7-release-gates.ps1'
    $gateAst = [System.Management.Automation.Language.Parser]::ParseFile($gateScript, [ref]$null, [ref]$null)
    $assignment = @($gateAst.FindAll({ param($node)
        $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
        $node.Left.Extent.Text -eq '$passed'
    }, $true))
    if ($assignment.Count -ne 1) { throw 'Release aggregate must have one passed-gate filter.' }
    $filter = @($assignment[0].Right.FindAll({ param($node)
        $node -is [System.Management.Automation.Language.ScriptBlockExpressionAst]
    }, $true))
    if ($filter.Count -ne 1) { throw 'Release aggregate filter is ambiguous.' }
    $predicate = $filter[0].ScriptBlock.GetScriptBlock()
    $exclusions = [pscustomobject]@{ gates = @('browser_media_push') }
    foreach ($case in @(
        @{ id = 'browser_media_push'; result = 'passed'; exit = 0; expected = 5 },
        @{ id = 'browser_media_push'; result = 'excluded'; exit = 0; expected = 5 },
        @{ id = 'browser'; result = 'excluded'; exit = 0; expected = 4 },
        @{ id = 'browser_media_push'; result = 'failed'; exit = 0; expected = 4 },
        @{ id = 'performance'; result = 'passed'; exit = 1; expected = 4 },
        @{ id = 'browser_media_push'; result = 'excluded'; exit = 1; expected = 4 }
    )) {
        $gates = @('browser', 'performance', 'browser_media_push', 'installer', 'rollback') | ForEach-Object {
            [pscustomobject]@{ id = $_; result = 'passed'; exit_status = 0 }
        }
        $changed = $gates | Where-Object id -eq $case.id
        $changed.result = $case.result
        $changed.exit_status = $case.exit
        $accepted = @($gates | Where-Object -FilterScript $predicate)
        if ($accepted.Count -ne $case.expected) {
            throw "Release aggregate rejected a valid gate set or accepted a failed gate: $($case | ConvertTo-Json -Compress); accepted=$($accepted.Count)"
        }
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
