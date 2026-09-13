[CmdletBinding()]
param([string]$AssertScript = '')

$ErrorActionPreference = 'Stop'
if (-not $AssertScript) { $AssertScript = Join-Path $PSScriptRoot 'assert-v7-exact-main-workflows.ps1' }
$sha = 'a' * 40
$fixtureRuns = @(
    [pscustomobject]@{ name = 'v7 CI'; path = '.github/workflows/ci.yml'; event = 'push'; head_branch = 'main'; head_sha = $sha; run_number = 1; conclusion = 'success' },
    [pscustomobject]@{ name = 'v7 Candidate Package'; path = '.github/workflows/package-v7-candidate.yml'; event = 'workflow_dispatch'; head_branch = 'main'; head_sha = $sha; run_number = 1; conclusion = 'success' },
    [pscustomobject]@{ name = 'Maintenance Security'; path = '.github/workflows/maintenance-security.yml'; event = 'push'; head_branch = 'main'; head_sha = $sha; run_number = 1; conclusion = 'success' },
    [pscustomobject]@{ name = 'Rust Security'; path = '.github/workflows/rust-security.yml'; event = 'push'; head_branch = 'main'; head_sha = $sha; run_number = 1; conclusion = 'success' }
)

# Replace only the GitHub response; execute the real gate without network/builds.
function Invoke-RestMethod {
    param($Method, $Uri, $Headers)
    return @{ workflow_runs = $fixtureRuns }
}

$result = & $AssertScript -Repository 'fixture/repo' -Sha $sha -Token 'fixture' | ConvertFrom-Json
if (-not $result.passed) { throw 'Manual candidate on exact main SHA must pass.' }

foreach ($case in @(
    @{ Index = 1; Field = 'event'; Value = 'push' },
    @{ Index = 0; Field = 'event'; Value = 'workflow_dispatch' },
    @{ Index = 1; Field = 'head_sha'; Value = ('b' * 40) },
    @{ Index = 1; Field = 'head_branch'; Value = 'other' },
    @{ Index = 1; Field = 'path'; Value = '.github/workflows/other.yml' },
    @{ Index = 1; Field = 'conclusion'; Value = 'failure' }
)) {
    $run = $fixtureRuns[$case.Index]
    $original = $run.($case.Field)
    $run.($case.Field) = $case.Value
    $rejected = $false
    try { & $AssertScript -Repository 'fixture/repo' -Sha $sha -Token 'fixture' | Out-Null }
    catch {
        if ($_.Exception.Message -notmatch 'has not succeeded for main SHA') { throw }
        $rejected = $true
    } finally { $run.($case.Field) = $original }
    if (-not $rejected) { throw "Gate accepted invalid $($case.Field)=$($case.Value)." }
}
Write-Output 'PASS: manual candidate accepted; wrong event, SHA, branch, path and failure rejected.'
