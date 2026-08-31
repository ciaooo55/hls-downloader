[CmdletBinding(DefaultParameterSetName='Upgrade')]
param(
    [Parameter(Mandatory=$true, ParameterSetName='Upgrade')][string]$SourceDir,
    [Parameter(Mandatory=$true, ParameterSetName='Upgrade')][string]$TargetDir,
    [Parameter(Mandatory=$true, ParameterSetName='Rollback')][switch]$Rollback,
    [Parameter(Mandatory=$true, ParameterSetName='Rollback')][string]$RollbackDir
)

$ErrorActionPreference = 'Stop'
$preserved = @('data','downloads')

function Full([string]$Path) { [IO.Path]::GetFullPath($Path).TrimEnd('\','/') }
function Stop-V7Processes([string]$Root) {
    $prefix = (Full $Root) + [IO.Path]::DirectorySeparatorChar
    Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
        Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}
function Assert-AppImage([string]$Root) {
    $identities = @{}
    foreach ($name in @('HLSDownloader.exe','app','runtime')) {
        if (-not (Test-Path -LiteralPath (Join-Path $Root $name))) { throw "v7 portable image is missing ${name}: $Root" }
    }
    $provenancePath = Join-Path $Root 'app\resources\BUILD-PROVENANCE.json'
    $featureParityPath = Join-Path $Root 'app\resources\FEATURE-PARITY.json'
    if (-not (Test-Path -LiteralPath $provenancePath -PathType Leaf) -or
        -not (Test-Path -LiteralPath $featureParityPath -PathType Leaf)) {
        throw "v7 portable image is missing provenance or feature parity: $Root"
    }
    $provenance = Get-Content -LiteralPath $provenancePath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int]$provenance.schema -ne 1 -or
        [string]$provenance.product_version -ne '7.0.0' -or
        @('candidate', 'formal') -notcontains [string]$provenance.package_tier -or
        [string]$provenance.feature_parity_path -ne 'artifacts/v7-productization/feature-parity.json' -or
        [string]$provenance.source_commit -notmatch '^[0-9a-fA-F]{40}$' -or
        [string]$provenance.source_tree -notmatch '^[0-9a-fA-F]{40}$') {
        throw "v7 portable image provenance is invalid: $Root"
    }
    $featureHash = (Get-FileHash -LiteralPath $featureParityPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ([string]$provenance.feature_parity_sha256 -notmatch '^[0-9a-fA-F]{64}$' -or
        $featureHash -ne ([string]$provenance.feature_parity_sha256).ToLowerInvariant()) {
        throw "v7 portable image feature parity is not bound to its provenance: $Root"
    }
    foreach ($browser in @('Chromium', 'Firefox')) {
        $archive = Join-Path $Root "extensions\HLSDownloader-7.0.0-$browser.zip"
        if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
            throw "v7 portable image is missing the $browser extension archive: $archive"
        }
        $check = Join-Path ([IO.Path]::GetTempPath()) ("hls-v7-upgrade-extension-" + [guid]::NewGuid().ToString('n'))
        try {
            Expand-Archive -LiteralPath $archive -DestinationPath $check -Force
            $manifestPath = Join-Path $check 'manifest.json'
            if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
                throw "v7 $browser extension archive is missing manifest.json: $archive"
            }
            $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
            if ([string]$manifest.version -ne '7.0.0' -or [int]$manifest.manifest_version -ne 3) {
                throw "v7 $browser extension manifest is not version 7.0.0 Manifest V3: $archive"
            }
            $identity = if ($browser -eq 'Chromium') {
                [string]$manifest.key
            } else {
                [string]$manifest.browser_specific_settings.gecko.id
            }
            if ([String]::IsNullOrWhiteSpace($identity)) {
                throw "v7 $browser extension manifest has no store identity: $archive"
            }
            $identities[$browser] = $identity
        } finally {
            Remove-Item -LiteralPath $check -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
    return $identities
}
function Copy-Preserved([string]$From, [string]$To) {
    foreach ($name in $preserved) {
        $source = Join-Path $From $name
        if (-not (Test-Path -LiteralPath $source)) { continue }
        $destination = Join-Path $To $name
        if (Test-Path -LiteralPath $destination) { Remove-Item -LiteralPath $destination -Recurse -Force }
        Copy-Item -LiteralPath $source -Destination $destination -Recurse -Force
    }
}

if ($PSCmdlet.ParameterSetName -eq 'Rollback') {
    $target = Full $RollbackDir
    $backup = "$target.v7-backup"
    $current = "$target.v7-rollback-current"
    if (-not (Test-Path -LiteralPath $backup -PathType Container)) { throw "v7 rollback backup is missing: $backup" }
    if (Test-Path -LiteralPath $current) { throw "v7 rollback recovery path already exists; inspect before retrying: $current" }
    $currentIdentities = Assert-AppImage $target
    $backupIdentities = Assert-AppImage $backup
    foreach ($browser in @('Chromium', 'Firefox')) {
        if (-not [String]::Equals([string]$currentIdentities[$browser], [string]$backupIdentities[$browser], [StringComparison]::Ordinal)) {
            throw "v7 $browser extension identity differs between the current and rollback images; rollback refused."
        }
    }
    Stop-V7Processes $target
    $targetMoved = $false
    $backupMoved = $false
    try {
        Move-Item -LiteralPath $target -Destination $current
        $targetMoved = $true
        Move-Item -LiteralPath $backup -Destination $target
        $backupMoved = $true
        Copy-Preserved $current $target
        Remove-Item -LiteralPath $current -Recurse -Force
        Write-Host "v7 portable rollback completed: $target"
    } catch {
        if ($backupMoved -and (Test-Path -LiteralPath $target)) {
            Move-Item -LiteralPath $target -Destination $backup -Force -ErrorAction SilentlyContinue
        }
        if ($targetMoved -and (Test-Path -LiteralPath $current) -and -not (Test-Path -LiteralPath $target)) {
            Move-Item -LiteralPath $current -Destination $target -Force -ErrorAction SilentlyContinue
        }
        throw
    }
    exit 0
}

$source = Full $SourceDir
$target = Full $TargetDir
$sourceIdentities = Assert-AppImage $source
$targetIdentities = Assert-AppImage $target
foreach ($browser in @('Chromium', 'Firefox')) {
    if (-not [String]::Equals([string]$sourceIdentities[$browser], [string]$targetIdentities[$browser], [StringComparison]::Ordinal)) {
        throw "v7 $browser extension identity differs from the installed image; upgrade refused."
    }
}
if ([String]::Equals($source, $target, [StringComparison]::OrdinalIgnoreCase)) { throw 'v7 source and target must be different directories' }
$backup = "$target.v7-backup"
if (Test-Path -LiteralPath $backup) { throw "v7 upgrade backup already exists; finalize or rollback first: $backup" }
Stop-V7Processes $target
Move-Item -LiteralPath $target -Destination $backup
try {
    Move-Item -LiteralPath $source -Destination $target
    Copy-Preserved $backup $target
    Write-Host "v7 portable upgrade completed: $target"
} catch {
    if (Test-Path -LiteralPath $target) { Move-Item -LiteralPath $target -Destination $source -Force -ErrorAction SilentlyContinue }
    if (Test-Path -LiteralPath $backup) { Move-Item -LiteralPath $backup -Destination $target -Force -ErrorAction SilentlyContinue }
    throw
}
