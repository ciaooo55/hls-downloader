[CmdletBinding()]
param(
    [string]$FeatureParityPath = '',
    [switch]$RequireCanonicalComplete,
    [switch]$RequireNoBlocked,
    [switch]$RequireReleaseReady,
    [switch]$RequireCleanWorktree,
    [ValidateSet('candidate', 'formal')]
    [string]$PackageTier = '',
    [string]$ProvenancePath = '',
    [string]$ReleaseEvidencePath = ''
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

if ([int]$json.schema -ne 1) {
    $errors.Add("Unsupported feature parity schema: $($json.schema)")
}
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
$canonicalFeatureIds = @(
    'architecture.single_core',
    'workbench.geometry',
    'workbench.foundation_component_architecture',
    'tasks.selection_keyboard_queue',
    'tasks.named_queue_profiles',
    'tasks.details_logs_speed_connections',
    'tasks.refresh_signed_url',
    'create.protocols_and_recognition',
    'import.file_picker_and_drop',
    'export.normalized_task_list',
    'import.exported_task_json',
    'workbench.automatic_responsive_layout',
    'media.hls_transfer_parity',
    'media.local_player_controls',
    'media.cast_dlna_chromecast',
    'media.lan_share',
    'media.tvbox_push',
    'media.cast_and_player_concurrency',
    'torrent.multi_file_selection',
    'settings.full_migration',
    'updates.confirmed_download',
    'browser.request_context_replay',
    'browser.takeover_and_recovery',
    'browser.media_push_device_selection',
    'browser.hot_confirmation_process',
    'accessibility.automation',
    'performance.release_thresholds',
    'package.install_upgrade_rollback'
)
$actualFeatureIds = @($features | ForEach-Object { [string]$_.id } | Sort-Object)
if (($actualFeatureIds -join "`n") -ne (($canonicalFeatureIds | Sort-Object) -join "`n")) {
    $errors.Add('Feature IDs do not match the canonical v7 feature set.')
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
$requiresCanonical = $RequireCanonicalComplete -or $RequireReleaseReady -or $RequireCleanWorktree
if ($requiresCanonical -and -not $path.Equals($canonicalPath, [StringComparison]::OrdinalIgnoreCase)) {
    $errors.Add("Package validation must use the canonical feature parity matrix: $canonicalPath")
}
if ($PackageTier -eq 'candidate') {
    if (-not $RequireNoBlocked -or -not $RequireCleanWorktree) {
        $errors.Add('Candidate validation must require no blocked features and a clean Git worktree.')
    }
    if ($RequireCanonicalComplete -or $RequireReleaseReady) {
        $errors.Add('Candidate validation cannot use formal-only release gates.')
    }
}
if ($PackageTier -eq 'formal') {
    if (-not $RequireCanonicalComplete -or -not $RequireReleaseReady -or -not $RequireCleanWorktree) {
        $errors.Add('Formal validation must require canonical complete, release_ready, and a clean Git worktree.')
    }
}
if ($RequireCanonicalComplete) {
    if ($features.Count -ne 28 -or $verified.Count -ne 28) {
        $errors.Add("Canonical feature parity must be complete: expected 28/28 verified, got $($verified.Count)/$($features.Count).")
    }
}
if ($RequireNoBlocked -and $blocked.Count -ne 0) {
    $incomplete = @($features | Where-Object status -eq 'blocked' | ForEach-Object { "$($_.id)=$($_.status)" })
    $errors.Add("Blocked features are not allowed for candidate packaging: $($incomplete -join ', ')")
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

$featureHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
$commit = Invoke-Git @('rev-parse', 'HEAD')
$tree = Invoke-Git @('rev-parse', 'HEAD^{tree}')
$branch = Invoke-Git @('branch', '--show-current')
$tag = Invoke-Git @('describe', '--exact-match', '--tags', 'HEAD') -AllowFailure
$worktreeStatus = Invoke-Git @('status', '--porcelain=v1', '--untracked-files=all')
$releaseEvidence = $null
$releaseEvidenceFullPath = $null
if ($RequireReleaseReady) {
    $releaseEvidenceFullPath = Resolve-RepositoryPath $ReleaseEvidencePath 'artifacts\v7-productization\release-evidence.json'
    $repoPrefix = $repo.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $releaseEvidenceFullPath.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        $errors.Add("Release evidence must stay inside the repository: $releaseEvidenceFullPath")
    } elseif (-not (Test-Path -LiteralPath $releaseEvidenceFullPath -PathType Leaf)) {
        $errors.Add("Release evidence is missing: $releaseEvidenceFullPath")
    } else {
        try {
            $releaseEvidence = [IO.File]::ReadAllText($releaseEvidenceFullPath, $utf8NoBom) | ConvertFrom-Json
        } catch {
            $errors.Add("Release evidence is not valid JSON: $($_.Exception.Message)")
        }
    }
    if ($null -ne $releaseEvidence) {
        if ([int]$releaseEvidence.schema -ne 1) { $errors.Add("Unsupported release evidence schema: $($releaseEvidence.schema)") }
        if ([string]$releaseEvidence.product_version -ne '7.0.0') { $errors.Add("Release evidence product version is not 7.0.0: $($releaseEvidence.product_version)") }
        if ([string]$releaseEvidence.source_commit -ne $commit) { $errors.Add("Release evidence source commit does not match HEAD: $($releaseEvidence.source_commit) != $commit") }
        if ([string]$releaseEvidence.source_tree -ne $tree) { $errors.Add("Release evidence source tree does not match HEAD: $($releaseEvidence.source_tree) != $tree") }

        $candidateManifestPath = [string]$releaseEvidence.candidate_artifact_manifest.path
        $candidateManifestHash = [string]$releaseEvidence.candidate_artifact_manifest.sha256
        $candidateManifestFullPath = if ([String]::IsNullOrWhiteSpace($candidateManifestPath)) { '' } else { Resolve-RepositoryPath $candidateManifestPath '' }
        if ([String]::IsNullOrWhiteSpace($candidateManifestFullPath) -or -not $candidateManifestFullPath.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            $errors.Add('Release evidence candidate artifact manifest path is missing or outside the repository.')
        } elseif (-not (Test-Path -LiteralPath $candidateManifestFullPath -PathType Leaf)) {
            $errors.Add("Candidate artifact manifest is missing: $candidateManifestFullPath")
        } else {
            $actualCandidateManifestHash = (Get-FileHash -LiteralPath $candidateManifestFullPath -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($candidateManifestHash -notmatch '^[0-9a-fA-F]{64}$' -or $actualCandidateManifestHash -ne $candidateManifestHash.ToLowerInvariant()) {
                $errors.Add("Candidate artifact manifest SHA-256 mismatch: $actualCandidateManifestHash != $candidateManifestHash")
            }
            try {
                $candidateManifest = [IO.File]::ReadAllText($candidateManifestFullPath, $utf8NoBom) | ConvertFrom-Json
                if ([int]$candidateManifest.schema -ne 1 -or [string]$candidateManifest.product_version -ne '7.0.0' -or [string]$candidateManifest.package_tier -ne 'candidate') {
                    $errors.Add('Candidate artifact manifest identity is not v7.0.0 candidate.')
                }
                if ([string]$candidateManifest.source_commit -ne $commit -or [string]$candidateManifest.source_tree -ne $tree) {
                    $errors.Add('Candidate artifact manifest is not from the current source commit and tree.')
                }
                if ([string]$candidateManifest.feature_parity_sha256 -ne $featureHash) {
                    $errors.Add('Candidate artifact manifest is not bound to the current feature parity matrix.')
                }
                $manifestRoot = [IO.Path]::GetDirectoryName($candidateManifestFullPath).TrimEnd('\', '/')
                $candidateFeaturePath = [IO.Path]::GetFullPath((Join-Path $manifestRoot ([string]$candidateManifest.feature_parity_path)))
                if (-not $candidateFeaturePath.StartsWith($manifestRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
                    -not (Test-Path -LiteralPath $candidateFeaturePath -PathType Leaf) -or
                    (Get-FileHash -LiteralPath $candidateFeaturePath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $featureHash) {
                    $errors.Add('Candidate feature parity artifact is missing, outside its directory, or stale.')
                }
                $candidatePortablePath = ''
                foreach ($artifactName in @('exe', 'msi', 'portable')) {
                    $artifact = $candidateManifest.artifacts.$artifactName
                    if ($null -eq $artifact -or [String]::IsNullOrWhiteSpace([string]$artifact.path)) {
                        $errors.Add("Candidate artifact manifest is missing the $artifactName entry.")
                        continue
                    }
                    $artifactPath = [IO.Path]::GetFullPath((Join-Path $manifestRoot ([string]$artifact.path)))
                    if (-not $artifactPath.StartsWith($manifestRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or -not (Test-Path -LiteralPath $artifactPath -PathType Leaf)) {
                        $errors.Add("Candidate $artifactName artifact is missing or outside its artifact directory.")
                    } else {
                        $actualArtifactHash = (Get-FileHash -LiteralPath $artifactPath -Algorithm SHA256).Hash.ToLowerInvariant()
                        if ([string]$artifact.sha256 -notmatch '^[0-9a-fA-F]{64}$' -or $actualArtifactHash -ne ([string]$artifact.sha256).ToLowerInvariant()) {
                            $errors.Add("Candidate $artifactName artifact SHA-256 mismatch.")
                        }
                        if ($artifactName -eq 'portable') { $candidatePortablePath = $artifactPath }
                    }
                }
                $extensionNames = @($candidateManifest.extensions.PSObject.Properties.Name | Sort-Object)
                if (($extensionNames -join "`n") -ne ((@('Chromium', 'Firefox') | Sort-Object) -join "`n")) {
                    $errors.Add('Candidate artifact manifest must contain exactly Chromium and Firefox extension evidence.')
                }
                if (-not [String]::IsNullOrWhiteSpace($candidatePortablePath)) {
                    $portableCheck = Join-Path $repo ('artifacts\v7-productization\.release-verify-' + [guid]::NewGuid().ToString('n'))
                    try {
                        Expand-Archive -LiteralPath $candidatePortablePath -DestinationPath $portableCheck -Force
                        $portableRoot = Join-Path $portableCheck 'HLSDownloader'
                        foreach ($extensionName in @('Chromium', 'Firefox')) {
                            $extension = $candidateManifest.extensions.$extensionName
                            $extensionRelativePath = [string]$extension.path
                            if ([string]$extension.version -ne '7.0.0' -or
                                [String]::IsNullOrWhiteSpace($extensionRelativePath) -or
                                [string]$extension.sha256 -notmatch '^[0-9a-fA-F]{64}$') {
                                $errors.Add("Candidate $extensionName extension evidence is missing its path, v7.0.0 identity, or SHA-256.")
                                continue
                            }
                            $extensionPath = [IO.Path]::GetFullPath((Join-Path $portableRoot $extensionRelativePath))
                            if (-not $extensionPath.StartsWith($portableRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
                                -not (Test-Path -LiteralPath $extensionPath -PathType Leaf)) {
                                $errors.Add("Candidate $extensionName extension archive is missing or outside the Portable root.")
                                continue
                            }
                            if ((Get-FileHash -LiteralPath $extensionPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne ([string]$extension.sha256).ToLowerInvariant()) {
                                $errors.Add("Candidate $extensionName extension archive SHA-256 mismatch.")
                            }
                            $extensionCheck = Join-Path $portableCheck ("extension-$extensionName")
                            Expand-Archive -LiteralPath $extensionPath -DestinationPath $extensionCheck -Force
                            $extensionManifest = Get-Content -LiteralPath (Join-Path $extensionCheck 'manifest.json') -Raw -Encoding UTF8 | ConvertFrom-Json
                            if ([string]$extensionManifest.version -ne '7.0.0') {
                                $errors.Add("Candidate $extensionName extension manifest version is not 7.0.0.")
                            }
                        }
                    } finally {
                        Remove-Item -LiteralPath $portableCheck -Recurse -Force -ErrorAction SilentlyContinue
                    }
                }
            } catch {
                $errors.Add("Candidate artifact manifest validation failed: $($_.Exception.Message)")
            }
        }

        $requiredGateIds = @('browser', 'performance', 'installer', 'rollback')
        $gates = @($releaseEvidence.gates)
        if ((@($gates.id | Sort-Object -Unique) -join "`n") -ne (($requiredGateIds | Sort-Object) -join "`n")) {
            $errors.Add('Release evidence must contain exactly browser, performance, installer, and rollback gates.')
        }
        foreach ($gate in $gates) {
            $gateId = [string]$gate.id
            foreach ($field in @('command', 'input', 'result')) {
                if ([String]::IsNullOrWhiteSpace([string]$gate.$field)) { $errors.Add("Release gate '$gateId' is missing '$field'.") }
            }
            if ($gate.PSObject.Properties.Name -notcontains 'output' -or $gate.output -isnot [string]) {
                $errors.Add("Release gate '$gateId' output must be present as a string.")
            }
            if ($gate.PSObject.Properties.Name -notcontains 'exit_status') {
                $errors.Add("Release gate '$gateId' is missing 'exit_status'.")
            } elseif ($gate.exit_status -isnot [int] -and $gate.exit_status -isnot [long]) {
                $errors.Add("Release gate '$gateId' exit_status must be an integer.")
            }
            if ([string]$gate.result -ne 'passed' -or [int]$gate.exit_status -ne 0) {
                $errors.Add("Release gate '$gateId' did not pass with exit status 0.")
            }
            if ([string]$gate.candidate_artifact_manifest_sha256 -notmatch '^[0-9a-fA-F]{64}$' -or
                ([string]$gate.candidate_artifact_manifest_sha256).ToLowerInvariant() -ne $candidateManifestHash.ToLowerInvariant()) {
                $errors.Add("Release gate '$gateId' is not bound to the candidate artifact manifest.")
            }
            $reportPath = [string]$gate.report.path
            $reportHash = [string]$gate.report.sha256
            $reportFullPath = if ([String]::IsNullOrWhiteSpace($reportPath)) { '' } else { Resolve-RepositoryPath $reportPath '' }
            if ([String]::IsNullOrWhiteSpace($reportFullPath) -or -not $reportFullPath.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
                $errors.Add("Release gate '$gateId' report path is missing or outside the repository.")
            } elseif (-not (Test-Path -LiteralPath $reportFullPath -PathType Leaf)) {
                $errors.Add("Release gate '$gateId' report is missing: $reportFullPath")
            } else {
                $actualReportHash = (Get-FileHash -LiteralPath $reportFullPath -Algorithm SHA256).Hash.ToLowerInvariant()
                if ($reportHash -notmatch '^[0-9a-fA-F]{64}$' -or $actualReportHash -ne $reportHash.ToLowerInvariant()) {
                    $errors.Add("Release gate '$gateId' report SHA-256 mismatch.")
                }
                try {
                    $report = [IO.File]::ReadAllText($reportFullPath, $utf8NoBom) | ConvertFrom-Json
                    if ([int]$report.schema -ne 1 -or [string]$report.gate_id -ne $gateId) {
                        $errors.Add("Release gate '$gateId' report identity is invalid.")
                    }
                    if ([string]$report.product_version -ne '7.0.0' -or [string]$report.source_commit -ne $commit -or [string]$report.source_tree -ne $tree) {
                        $errors.Add("Release gate '$gateId' report is not from the current v7.0.0 source.")
                    }
                    if ([string]$report.candidate_artifact_manifest_sha256 -notmatch '^[0-9a-fA-F]{64}$' -or
                        ([string]$report.candidate_artifact_manifest_sha256).ToLowerInvariant() -ne $candidateManifestHash.ToLowerInvariant()) {
                        $errors.Add("Release gate '$gateId' report is not bound to the candidate artifact manifest.")
                    }
                    if ($report.PSObject.Properties.Name -notcontains 'output' -or $report.output -isnot [string]) {
                        $errors.Add("Release gate '$gateId' report output must be present as a string.")
                    }
                    foreach ($field in @('command', 'input', 'output', 'result')) {
                        if ([string]$report.$field -ne [string]$gate.$field) {
                            $errors.Add("Release gate '$gateId' report field '$field' does not match the evidence manifest.")
                        }
                    }
                    if ($report.PSObject.Properties.Name -notcontains 'exit_status') {
                        $errors.Add("Release gate '$gateId' report is missing 'exit_status'.")
                    } elseif ($report.exit_status -isnot [int] -and $report.exit_status -isnot [long]) {
                        $errors.Add("Release gate '$gateId' report exit_status must be an integer.")
                    }
                    if ([int]$report.exit_status -ne [int]$gate.exit_status) {
                        $errors.Add("Release gate '$gateId' report exit status does not match the evidence manifest.")
                    }
                } catch {
                    $errors.Add("Release gate '$gateId' report is not a valid result envelope: $($_.Exception.Message)")
                }
            }
        }
    }
}
if ($RequireCleanWorktree) {
    $trackedAudit = Invoke-Git @(
        'ls-files', '--error-unmatch', '--', 'artifacts/v7-productization/feature-parity.json'
    ) -AllowFailure
    if ([String]::IsNullOrWhiteSpace($trackedAudit)) {
        $errors.Add('Canonical feature parity matrix must be tracked by Git before package validation.')
    }
}
if ($RequireCleanWorktree -and -not [String]::IsNullOrWhiteSpace($worktreeStatus)) {
    $dirtyCount = @($worktreeStatus -split "`n" | Where-Object { -not [String]::IsNullOrWhiteSpace($_) }).Count
    $errors.Add("Package validation requires a clean Git worktree; found $dirtyCount changed path(s).")
}

if ($errors.Count -gt 0) {
    throw ("V7 release gate failed:`n- " + ($errors -join "`n- "))
}

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
        package_tier = if ([String]::IsNullOrWhiteSpace($PackageTier)) { 'validation' } else { $PackageTier }
        release_ready = ($json.release_ready -eq $true)
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
    if ($RequireReleaseReady -and $null -ne $releaseEvidence) {
        $provenance.release_evidence_path = $releaseEvidenceFullPath.Substring($repoPrefix.Length).Replace('\', '/')
        $provenance.release_evidence_sha256 = (Get-FileHash -LiteralPath $releaseEvidenceFullPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $provenance.candidate_artifact_manifest = $releaseEvidence.candidate_artifact_manifest
        $provenance.release_gates = @($releaseEvidence.gates | ForEach-Object {
            [ordered]@{ id = [string]$_.id; result = [string]$_.result; exit_status = [int]$_.exit_status; report = $_.report }
        })
    }
    [IO.File]::WriteAllText($outputPath, ($provenance | ConvertTo-Json -Depth 8), $utf8NoBom)
}

Write-Output "FEATURE_PARITY=$verifiedPercent% ($($verified.Count)/$($features.Count) verified, $($partial.Count) partial, $($blocked.Count) blocked); COMMIT=$commit; TREE=$tree; SHA256=$featureHash"
