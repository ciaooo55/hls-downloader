[CmdletBinding()]
param(
    [string]$FeatureParityPath = '',
    [switch]$RequireReleaseReady,
    [switch]$RequireCleanWorktree,
    [string]$ProvenancePath = ''
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Resolve-RepositoryPath([string]$Path, [string]$DefaultRelativePath) {
    $candidate = if ([String]::IsNullOrWhiteSpace($Path)) { $DefaultRelativePath } else { $Path }
    if ([IO.Path]::IsPathRooted($candidate)) {
        return [IO.Path]::GetFullPath($candidate)
    }
    return [IO.Path]::GetFullPath((Join-Path $repo $candidate))
}

function Invoke-Git([string[]]$Arguments, [switch]$AllowFailure) {
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        # Windows PowerShell 5.1 surfaces native stderr as ErrorRecord objects.
        # Keep expected probe failures local so the caller can classify them.
        $ErrorActionPreference = 'Continue'
        $output = @(& git -C $repo @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($exitCode -ne 0 -and -not $AllowFailure) {
        throw "git $($Arguments -join ' ') failed with exit $exitCode`: $($output -join [Environment]::NewLine)"
    }
    if ($exitCode -ne 0) { return '' }
    return ($output -join "`n").Trim()
}

$path = Resolve-RepositoryPath $FeatureParityPath 'artifacts\v7-productization\feature-parity.json'
$canonicalPath = Resolve-RepositoryPath '' 'artifacts\v7-productization\feature-parity.json'
if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
    throw "Canonical feature parity matrix is missing: $path"
}
$json = [IO.File]::ReadAllText($path, $utf8NoBom) | ConvertFrom-Json
$errors = New-Object 'System.Collections.Generic.List[string]'

if ($json.product_version -ne '7.0.0') {
    $errors.Add("Unexpected product version: $($json.product_version)")
}
$features = @($json.features)
if ($features.Count -eq 0) {
    $errors.Add('Feature parity matrix is empty.')
}
$requiredFields = @('id', 'status', 'v3_entry', 'compose_entry', 'core_command', 'state_event', 'verification')
$allowedStatuses = @('verified', 'partial', 'blocked')
foreach ($feature in $features) {
    foreach ($field in $requiredFields) {
        $value = $feature.$field
        if ($null -eq $value -or [String]::IsNullOrWhiteSpace([string]$value)) {
            $errors.Add("Feature '$($feature.id)' is missing '$field'.")
        }
    }
    if ($allowedStatuses -notcontains [string]$feature.status) {
        $errors.Add("Feature '$($feature.id)' has invalid status '$($feature.status)'.")
    }
}
if (@($features.id | Sort-Object -Unique).Count -ne $features.Count) {
    $errors.Add('Feature IDs must be unique.')
}

$verified = @($features | Where-Object status -eq 'verified')
$partial = @($features | Where-Object status -eq 'partial')
$blocked = @($features | Where-Object status -eq 'blocked')
$verifiedPercent = if ($features.Count -eq 0) { 0.0 } else { [Math]::Round(100.0 * $verified.Count / $features.Count, 1) }
if (
    [int]$json.summary.total -ne $features.Count -or
    [int]$json.summary.verified -ne $verified.Count -or
    [int]$json.summary.partial -ne $partial.Count -or
    [int]$json.summary.blocked -ne $blocked.Count -or
    [double]$json.summary.verified_percent -ne $verifiedPercent
) {
    $errors.Add('Feature parity summary does not match the feature records.')
}
if ($RequireReleaseReady) {
    if ($json.release_ready -ne $true) {
        $errors.Add('release_ready must be true before formal packaging.')
    }
    if ($verified.Count -ne $features.Count) {
        $incomplete = @($features | Where-Object status -ne 'verified' | ForEach-Object { "$($_.id)=$($_.status)" })
        $errors.Add("Unverified features: $($incomplete -join ', ')")
    }
}

$commit = Invoke-Git @('rev-parse', 'HEAD')
$tree = Invoke-Git @('rev-parse', 'HEAD^{tree}')
$branch = Invoke-Git @('branch', '--show-current')
$tag = Invoke-Git @('describe', '--exact-match', '--tags', 'HEAD') -AllowFailure
$worktreeStatus = Invoke-Git @('status', '--porcelain=v1', '--untracked-files=all')
if ($RequireCleanWorktree) {
    if (-not $path.Equals($canonicalPath, [StringComparison]::OrdinalIgnoreCase)) {
        $errors.Add("Formal packaging must use the canonical feature parity matrix: $canonicalPath")
    }
    $trackedAudit = Invoke-Git @(
        'ls-files', '--error-unmatch', '--', 'artifacts/v7-productization/feature-parity.json'
    ) -AllowFailure
    if ([String]::IsNullOrWhiteSpace($trackedAudit)) {
        $errors.Add('Canonical feature parity matrix must be tracked by Git before formal packaging.')
    }
}
if ($RequireCleanWorktree -and -not [String]::IsNullOrWhiteSpace($worktreeStatus)) {
    $dirtyCount = @($worktreeStatus -split "`n" | Where-Object { -not [String]::IsNullOrWhiteSpace($_) }).Count
    $errors.Add("Formal packaging requires a clean Git worktree; found $dirtyCount changed path(s).")
}

if ($errors.Count -gt 0) {
    throw ("V7 release gate failed:`n- " + ($errors -join "`n- "))
}

$featureHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
if (-not [String]::IsNullOrWhiteSpace($ProvenancePath)) {
    $outputPath = Resolve-RepositoryPath $ProvenancePath 'artifacts\v7-productization\package\BUILD-PROVENANCE.json'
    $outputDir = [IO.Path]::GetDirectoryName($outputPath)
    New-Item -ItemType Directory -Force -Path $outputDir | Out-Null
    $repoPrefix = $repo.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $relativeFeaturePath = if ($path.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        $path.Substring($repoPrefix.Length).Replace('\', '/')
    } else {
        $path.Replace('\', '/')
    }
    $provenance = [ordered]@{
        schema = 1
        product_version = [string]$json.product_version
        source_commit = $commit
        source_tree = $tree
        source_branch = $branch
        source_tag = $tag
        git_worktree_clean = [String]::IsNullOrWhiteSpace($worktreeStatus)
        feature_parity_path = $relativeFeaturePath
        feature_parity_sha256 = $featureHash
        feature_summary = [ordered]@{
            verified = $verified.Count
            partial = $partial.Count
            blocked = $blocked.Count
            total = $features.Count
        }
        generated_at_utc = [DateTime]::UtcNow.ToString('o')
    }
    [IO.File]::WriteAllText($outputPath, ($provenance | ConvertTo-Json -Depth 8), $utf8NoBom)
}

Write-Output "FEATURE_PARITY=$verifiedPercent% ($($verified.Count)/$($features.Count) verified, $($partial.Count) partial, $($blocked.Count) blocked); COMMIT=$commit; TREE=$tree; SHA256=$featureHash"
