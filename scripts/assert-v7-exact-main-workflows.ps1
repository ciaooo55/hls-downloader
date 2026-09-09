[CmdletBinding()]
param(
    [string]$Repository = $env:GITHUB_REPOSITORY,
    [string]$Sha = $env:GITHUB_SHA,
    [string]$Token = $env:HLS_V7_ACTIONS_TOKEN
)

$ErrorActionPreference = 'Stop'

if ([String]::IsNullOrWhiteSpace($Repository) -or $Repository -notmatch '^[^/]+/[^/]+$') {
    throw "A valid owner/repository is required: $Repository"
}
if ([String]::IsNullOrWhiteSpace($Sha) -or $Sha -notmatch '^[0-9a-fA-F]{40}$') {
    throw "A full 40-character Git SHA is required: $Sha"
}
if ([String]::IsNullOrWhiteSpace($Token)) {
    throw 'An Actions-read GitHub token is required.'
}

$headers = @{
    Authorization = "Bearer $Token"
    Accept = 'application/vnd.github+json'
    'X-GitHub-Api-Version' = '2022-11-28'
    'User-Agent' = 'hls-downloader-exact-main-workflow-assertion'
}
$uri = "https://api.github.com/repos/$Repository/actions/runs?head_sha=$Sha&status=completed&per_page=100"
$runs = Invoke-RestMethod -Method Get -Uri $uri -Headers $headers
$requiredWorkflows = @(
    [pscustomobject]@{ Name = 'v7 CI'; Path = '.github/workflows/ci.yml' },
    [pscustomobject]@{ Name = 'v7 Candidate Package'; Path = '.github/workflows/package-v7-candidate.yml' },
    [pscustomobject]@{ Name = 'Maintenance Security'; Path = '.github/workflows/maintenance-security.yml' },
    [pscustomobject]@{ Name = 'Rust Security'; Path = '.github/workflows/rust-security.yml' }
)

foreach ($required in $requiredWorkflows) {
    $match = @(
        $runs.workflow_runs |
            Where-Object {
                $_.name -eq $required.Name -and
                $_.path -eq $required.Path -and
                $_.event -eq 'push' -and
                $_.head_branch -eq 'main' -and
                $_.head_sha -eq $Sha
            } |
            Sort-Object run_number -Descending |
            Select-Object -First 1
    )
    if ($match.Count -ne 1 -or $match[0].conclusion -ne 'success') {
        throw "$($required.Name) at $($required.Path) has not succeeded for main SHA $Sha."
    }
}

Write-Output ([ordered]@{
    schema = 1
    passed = $true
    repository = $Repository
    source_commit = $Sha.ToLowerInvariant()
    workflows = @($requiredWorkflows | ForEach-Object { [ordered]@{ name = $_.Name; path = $_.Path } })
} | ConvertTo-Json -Depth 4 -Compress)
