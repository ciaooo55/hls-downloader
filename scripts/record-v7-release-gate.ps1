[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)]
    [ValidateSet('browser', 'performance', 'installer', 'rollback')]
    [string]$GateId,
    [Parameter(Mandatory=$true)][ValidateNotNullOrEmpty()][string]$Command,
    [Parameter(Mandatory=$true)][ValidateNotNullOrEmpty()][string]$Input,
    [string]$CandidateManifestPath = '',
    [string]$EvidencePath = ''
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$repoPrefix = $repo.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Resolve-RepositoryPath([string]$Path, [string]$DefaultRelativePath) {
    $candidate = if ([String]::IsNullOrWhiteSpace($Path)) { $DefaultRelativePath } else { $Path }
    $full = if ([IO.Path]::IsPathRooted($candidate)) {
        [IO.Path]::GetFullPath($candidate)
    } else {
        [IO.Path]::GetFullPath((Join-Path $repo $candidate))
    }
    if (-not $full.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Release evidence path must stay inside the repository: $full"
    }
    return $full
}

function Invoke-Git([string[]]$Arguments) {
    $value = (& git -C $repo @Arguments 2>&1 | ForEach-Object { $_.ToString() }) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw "git $($Arguments -join ' ') failed: $value" }
    return $value.Trim()
}

function Write-JsonAtomic([string]$Path, $Value) {
    New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($Path)) | Out-Null
    $temporary = "$Path.tmp"
    [IO.File]::WriteAllText($temporary, ($Value | ConvertTo-Json -Depth 10), $utf8NoBom)
    Move-Item -LiteralPath $temporary -Destination $Path -Force
}

$manifestPath = Resolve-RepositoryPath $CandidateManifestPath 'artifacts\v7-productization\candidate\ARTIFACT-MANIFEST.json'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Candidate artifact manifest is missing: $manifestPath"
}
$manifest = [IO.File]::ReadAllText($manifestPath, $utf8NoBom) | ConvertFrom-Json
$commit = Invoke-Git @('rev-parse', 'HEAD')
$tree = Invoke-Git @('rev-parse', 'HEAD^{tree}')
if ([int]$manifest.schema -ne 1 -or
    [string]$manifest.product_version -ne '7.0.0' -or
    [string]$manifest.package_tier -ne 'candidate' -or
    [string]$manifest.source_commit -ne $commit -or
    [string]$manifest.source_tree -ne $tree) {
    throw 'Candidate artifact manifest is not a v7.0.0 candidate from the current source commit and tree.'
}
$manifestHash = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
$manifestRelativePath = $manifestPath.Substring($repoPrefix.Length).Replace('\', '/')

$previousErrorActionPreference = $ErrorActionPreference
try {
    $ErrorActionPreference = 'Continue'
    $captured = @(& powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command $Command 2>&1 |
        ForEach-Object { $_.ToString() })
    $exitStatus = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $previousErrorActionPreference
}
$output = $captured -join [Environment]::NewLine
$result = if ($exitStatus -eq 0) { 'passed' } else { 'failed' }
$report = [ordered]@{
    schema = 1
    gate_id = $GateId
    product_version = '7.0.0'
    source_commit = $commit
    source_tree = $tree
    candidate_artifact_manifest_sha256 = $manifestHash
    command = $Command
    input = $Input
    output = $output
    result = $result
    exit_status = [int]$exitStatus
    recorded_at_utc = [DateTime]::UtcNow.ToString('o')
}
$reportPath = Resolve-RepositoryPath '' "artifacts\v7-productization\release-evidence\$GateId.json"
Write-JsonAtomic $reportPath $report
$reportHash = (Get-FileHash -LiteralPath $reportPath -Algorithm SHA256).Hash.ToLowerInvariant()
$reportRelativePath = $reportPath.Substring($repoPrefix.Length).Replace('\', '/')

$evidenceFullPath = Resolve-RepositoryPath $EvidencePath 'artifacts\v7-productization\release-evidence.json'
$existingGates = @()
if (Test-Path -LiteralPath $evidenceFullPath -PathType Leaf) {
    $existing = [IO.File]::ReadAllText($evidenceFullPath, $utf8NoBom) | ConvertFrom-Json
    if ([int]$existing.schema -eq 1 -and
        [string]$existing.source_commit -eq $commit -and
        [string]$existing.source_tree -eq $tree -and
        [string]$existing.candidate_artifact_manifest.sha256 -eq $manifestHash) {
        $existingGates = @($existing.gates | Where-Object { [string]$_.id -ne $GateId })
    }
}
$gate = [ordered]@{
    id = $GateId
    command = $Command
    input = $Input
    output = $output
    result = $result
    exit_status = [int]$exitStatus
    candidate_artifact_manifest_sha256 = $manifestHash
    report = [ordered]@{ path = $reportRelativePath; sha256 = $reportHash }
}
$evidence = [ordered]@{
    schema = 1
    product_version = '7.0.0'
    source_commit = $commit
    source_tree = $tree
    candidate_artifact_manifest = [ordered]@{ path = $manifestRelativePath; sha256 = $manifestHash }
    gates = @($existingGates + $gate | Sort-Object { [string]$_.id })
}
Write-JsonAtomic $evidenceFullPath $evidence

$captured | Write-Output
Write-Output "RELEASE_GATE=$GateId; RESULT=$result; EXIT_STATUS=$exitStatus; REPORT=$reportRelativePath"
exit $exitStatus
