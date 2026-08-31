[CmdletBinding()]
param(
    [string]$ArtifactManifestPath = '',
    [string]$TargetDir = 'E:\h',
    [string]$UpgradeNote = ''
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$repoPrefix = $repo.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
$installRoot = [IO.Path]::GetFullPath('E:\h').TrimEnd('\', '/')
$target = [IO.Path]::GetFullPath($TargetDir).TrimEnd('\', '/')
if (-not [String]::Equals($target, $installRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Local install target must be exactly ${installRoot}: $target"
}
$artifactManifest = if ([String]::IsNullOrWhiteSpace($ArtifactManifestPath)) {
    Join-Path $repo 'artifacts\v7-productization\candidate\ARTIFACT-MANIFEST.json'
} else {
    [IO.Path]::GetFullPath($ArtifactManifestPath)
}
if (-not $artifactManifest.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase) -or
    -not (Test-Path -LiteralPath $artifactManifest -PathType Leaf)) {
    throw "v7 artifact manifest must exist inside this repository: $artifactManifest"
}
$artifact = Get-Content -LiteralPath $artifactManifest -Raw -Encoding UTF8 | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
if ([int]$artifact.schema -ne 1 -or
    [string]$artifact.product_version -ne '7.0.0' -or
    @('candidate', 'formal') -notcontains [string]$artifact.package_tier -or
    [string]$artifact.source_commit -ne $currentCommit -or
    [string]$artifact.source_tree -ne $currentTree) {
    throw 'v7 artifact manifest is not a current v7.0.0 candidate/formal package.'
}
$artifactRoot = [IO.Path]::GetDirectoryName($artifactManifest).TrimEnd('\', '/')
$portableEntry = $artifact.artifacts.portable
$portable = [IO.Path]::GetFullPath((Join-Path $artifactRoot ([string]$portableEntry.path)))
if (-not $portable.StartsWith($artifactRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
    -not (Test-Path -LiteralPath $portable -PathType Leaf)) {
    throw "v7 Portable artifact is missing or escaped its artifact directory: $portable"
}
$portableHash = (Get-FileHash -LiteralPath $portable -Algorithm SHA256).Hash.ToLowerInvariant()
if ([string]$portableEntry.sha256 -notmatch '^[0-9a-fA-F]{64}$' -or
    $portableHash -ne ([string]$portableEntry.sha256).ToLowerInvariant()) {
    throw 'v7 Portable artifact SHA-256 does not match ARTIFACT-MANIFEST.json.'
}
$installSourceStage = Join-Path $repo ('artifacts\v7-productization\.local-install-source-' + [guid]::NewGuid().ToString('n'))
try {
    Expand-Archive -LiteralPath $portable -DestinationPath $installSourceStage -Force
    $source = Join-Path $installSourceStage 'HLSDownloader'
foreach ($name in @(
    'HLSDownloader.exe',
    'app',
    'runtime',
    'app\resources\HLSDownloaderEngine.exe',
    'app\resources\HLSDownloaderNativeHost.exe',
    'app\resources\HLSDownloaderUpdater.exe',
    'app\resources\HLSDownloaderPresenter.exe',
    'app\resources\ffmpeg.exe',
    'app\resources\ffprobe.exe',
    'app\resources\libmpv-2.dll',
    'app\resources\BUILD-PROVENANCE.json',
    'app\resources\FEATURE-PARITY.json'
)) {
    if (-not (Test-Path -LiteralPath (Join-Path $source $name))) {
        throw "v7 local image is incomplete; missing ${name}: $source"
    }
}
$provenancePath = Join-Path $source 'app\resources\BUILD-PROVENANCE.json'
$featureParityPath = Join-Path $source 'app\resources\FEATURE-PARITY.json'
$provenance = Get-Content -LiteralPath $provenancePath -Raw -Encoding UTF8 | ConvertFrom-Json
if ([int]$provenance.schema -ne 1) {
    throw "v7 local image provenance schema is unsupported: $($provenance.schema)"
}
if ($provenance.product_version -ne '7.0.0') {
    throw "v7 local image provenance product_version is not 7.0.0: $($provenance.product_version)"
}
if (@('candidate', 'formal') -notcontains [string]$provenance.package_tier) {
    throw "v7 local image provenance package_tier is invalid: $($provenance.package_tier)"
}
if ([string]$provenance.package_tier -ne [string]$artifact.package_tier) {
    throw 'v7 local image provenance tier does not match ARTIFACT-MANIFEST.json.'
}
if ($provenance.source_commit -ne $currentCommit -or $provenance.source_tree -ne $currentTree) {
    throw "v7 local image provenance does not match this checkout: $($provenance.source_commit)/$($provenance.source_tree) != $currentCommit/$currentTree"
}
if ([string]$provenance.feature_parity_path -ne 'artifacts/v7-productization/feature-parity.json') {
    throw "v7 local image feature parity path is not canonical: $($provenance.feature_parity_path)"
}
$featureHash = (Get-FileHash -LiteralPath $featureParityPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ([string]$provenance.feature_parity_sha256 -ne $featureHash) {
    throw "v7 local image feature parity hash does not match provenance: $($provenance.feature_parity_sha256) != $featureHash"
}
$featureJson = Get-Content -LiteralPath $featureParityPath -Raw -Encoding UTF8 | ConvertFrom-Json
foreach ($field in @('total', 'verified', 'partial', 'blocked')) {
    if ([int]$provenance.feature_summary.$field -ne [int]$featureJson.summary.$field) {
        throw "v7 local image feature summary does not match the canonical matrix: $field"
    }
}
$identitySource = Get-Content -LiteralPath (Join-Path $repo 'extension\lib\storeIdentity.ts') -Raw -Encoding UTF8
$expectedChromiumKey = ([regex]::Match($identitySource, "CHROMIUM_PUBLIC_KEY = '([^']+)'" )).Groups[1].Value
$expectedFirefoxId = ([regex]::Match($identitySource, "FIREFOX_EXTENSION_ID = '([^']+)'" )).Groups[1].Value
if ([String]::IsNullOrWhiteSpace($expectedChromiumKey) -or [String]::IsNullOrWhiteSpace($expectedFirefoxId)) {
    throw 'Extension store identity constants are missing.'
}

function Assert-ExtensionArchive([string]$Archive, [string]$Browser) {
    if (-not (Test-Path -LiteralPath $Archive -PathType Leaf)) {
        throw "Packaged $Browser extension archive is missing: $Archive"
    }
    $check = Join-Path $installSourceStage (".extension-check-" + [guid]::NewGuid().ToString('n'))
    try {
        Expand-Archive -LiteralPath $Archive -DestinationPath $check -Force
        $manifestPath = Join-Path $check 'manifest.json'
        if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
            throw "Packaged $Browser extension archive is missing manifest.json."
        }
        $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($manifest.version -ne '7.0.0') {
            throw "Packaged $Browser extension version is not 7.0.0: $($manifest.version)"
        }
        if ([int]$manifest.manifest_version -ne 3) {
            throw "Packaged $Browser extension is not Manifest V3."
        }
        if ($Browser -eq 'Chromium' -and [string]$manifest.key -ne $expectedChromiumKey) {
            throw 'Packaged Chromium extension key does not match store identity.'
        }
        if ($Browser -eq 'Firefox' -and [string]$manifest.browser_specific_settings.gecko.id -ne $expectedFirefoxId) {
            throw 'Packaged Firefox extension id does not match store identity.'
        }
    } finally {
        Remove-Item -LiteralPath $check -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$sourceRoot = [IO.Path]::GetFullPath($source).TrimEnd('\', '/')
$extensionArchivePaths = @{}
foreach ($browser in @('Chromium', 'Firefox')) {
    $entry = $artifact.extensions.$browser
    if ([string]$entry.version -ne '7.0.0' -or
        [String]::IsNullOrWhiteSpace([string]$entry.path) -or
        [string]$entry.sha256 -notmatch '^[0-9a-fA-F]{64}$') {
        throw "v7 $browser extension entry is incomplete in ARTIFACT-MANIFEST.json."
    }
    $archive = [IO.Path]::GetFullPath((Join-Path $sourceRoot ([string]$entry.path)))
    if (-not $archive.StartsWith($sourceRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
        -not (Test-Path -LiteralPath $archive -PathType Leaf)) {
        throw "v7 $browser extension archive is missing or escaped the Portable root: $archive"
    }
    if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne ([string]$entry.sha256).ToLowerInvariant()) {
        throw "v7 $browser extension SHA-256 does not match ARTIFACT-MANIFEST.json."
    }
    Assert-ExtensionArchive $archive $browser
    $extensionArchivePaths[$browser] = $archive
}
$note = if ([String]::IsNullOrWhiteSpace($UpgradeNote)) {
    Join-Path $repo 'docs\v7-desktop-upgrade-note.md'
} else {
    [IO.Path]::GetFullPath($UpgradeNote)
}
if (-not $note.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase) -or
    -not (Test-Path -LiteralPath $note -PathType Leaf)) {
    throw "Upgrade note is missing: $note"
}

$stage = "$target.v7-stage"
$backup = "$target.v7-backup"
$desktopExtensionStage = "$target.v7-desktop-stage"
$desktopExtensionBackup = "$target.v7-desktop-backup"
$finalizeMarker = "$target.v7-finalize.json"
$ownerMarkerName = '.v7-install-owner.json'
$transactionNonce = [guid]::NewGuid().ToString('n')
function Test-OwnerMarker([string]$Path, [string]$ExpectedTarget, [string]$ExpectedNonce) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $false }
    try {
        $owner = Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
        return ([int]$owner.schema -eq 1 -and
            [string]$owner.target -eq $ExpectedTarget -and
            [string]$owner.nonce -eq $ExpectedNonce)
    } catch { return $false }
}
function Test-FinalizeMarker([string]$Path, [string]$ExpectedTarget) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf) -or
        -not (Test-Path -LiteralPath $ExpectedTarget -PathType Container)) { return $null }
    try {
        $marker = Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
        $installationPath = Join-Path $ExpectedTarget 'INSTALLATION.txt'
        $provenancePath = Join-Path $ExpectedTarget 'app\resources\BUILD-PROVENANCE.json'
        if ([int]$marker.schema -ne 1 -or
            [string]$marker.version -ne '7.0.0' -or
            -not [String]::Equals([string]$marker.target, $ExpectedTarget, [StringComparison]::OrdinalIgnoreCase) -or
            [string]$marker.nonce -notmatch '^[0-9a-f]{32}$' -or
            -not (Test-Path -LiteralPath $installationPath -PathType Leaf) -or
            [string]$marker.installation_sha256 -ne (Get-FileHash -LiteralPath $installationPath -Algorithm SHA256).Hash.ToLowerInvariant() -or
            -not (Test-Path -LiteralPath $provenancePath -PathType Leaf) -or
            [string]$marker.provenance_sha256 -ne (Get-FileHash -LiteralPath $provenancePath -Algorithm SHA256).Hash.ToLowerInvariant()) {
            return $null
        }
        return $marker
    } catch { return $null }
}
function Remove-OwnedDirectory([string]$Path, [string]$ExpectedTarget, [string]$ExpectedNonce) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $ownerPath = Join-Path $Path $ownerMarkerName
    if (-not (Test-OwnerMarker $ownerPath $ExpectedTarget $ExpectedNonce)) {
        throw "v7 finalize path is not owned by this install transaction: $Path"
    }
    Get-ChildItem -LiteralPath $Path -Force | Where-Object { $_.Name -ne $ownerMarkerName } |
        ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force }
    Remove-Item -LiteralPath $ownerPath -Force
    Remove-Item -LiteralPath $Path -Force
}
$priorFinalize = Test-FinalizeMarker $finalizeMarker $target
if (Test-Path -LiteralPath $finalizeMarker) {
    if ($null -eq $priorFinalize) {
        throw "v7 finalize marker is invalid or does not match the current E:\\h installation: $finalizeMarker"
    }
    $cleanup = @(
        @{ Path = $backup; Expected = ([bool]$priorFinalize.root_backup_expected) },
        @{ Path = $desktopExtensionBackup; Expected = ([bool]$priorFinalize.desktop_backup_expected) }
    )
    foreach ($item in $cleanup) {
        if (Test-Path -LiteralPath $item.Path) {
            $owner = Join-Path $item.Path $ownerMarkerName
            if (-not $item.Expected -or -not (Test-OwnerMarker $owner $target ([string]$priorFinalize.nonce))) {
                throw "v7 finalize path is not owned by this install transaction: $($item.Path)"
            }
        }
    }
    foreach ($item in $cleanup) {
        if (Test-Path -LiteralPath $item.Path) {
            Remove-OwnedDirectory $item.Path $target ([string]$priorFinalize.nonce)
        }
    }
    if ((Test-Path -LiteralPath $backup) -or (Test-Path -LiteralPath $desktopExtensionBackup)) {
        throw 'v7 previous install finalize cleanup is incomplete; retry after the locked path is released.'
    }
    Remove-Item -LiteralPath $finalizeMarker -Force
}
foreach ($candidate in @($stage, $backup)) {
    $full = [IO.Path]::GetFullPath($candidate)
    if (-not $full.StartsWith($installRoot + '.v7-', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Local install working path escaped ${installRoot}: $full"
    }
}
foreach ($candidate in @($desktopExtensionStage, $desktopExtensionBackup)) {
    $full = [IO.Path]::GetFullPath($candidate)
    if (-not $full.StartsWith($installRoot + '.v7-', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Desktop extension working path escaped ${installRoot}: $full"
    }
}
$hadPrevious = Test-Path -LiteralPath $target
if ($hadPrevious) {
    if (-not (Test-Path -LiteralPath $target -PathType Container)) {
        throw "Existing E:\h path is not a v7 installation directory: $target"
    }
    $existingProvenancePath = Join-Path $target 'app\resources\BUILD-PROVENANCE.json'
    $existingInstallationPath = Join-Path $target 'INSTALLATION.txt'
    if (-not (Test-Path -LiteralPath $existingProvenancePath -PathType Leaf) -or
        -not (Test-Path -LiteralPath $existingInstallationPath -PathType Leaf)) {
        throw "Existing E:\h directory is not an owned v7 installation: $target"
    }
    try {
        $existingProvenance = Get-Content -LiteralPath $existingProvenancePath -Raw -Encoding UTF8 | ConvertFrom-Json
    } catch {
        throw "Existing E:\h installation provenance is invalid: $existingProvenancePath"
    }
    $existingInstallHeader = Get-Content -LiteralPath $existingInstallationPath -TotalCount 1 -Encoding UTF8
    if ([int]$existingProvenance.schema -ne 1 -or
        [string]$existingProvenance.product_version -ne '7.0.0' -or
        @('candidate', 'formal') -notcontains [string]$existingProvenance.package_tier -or
        [string]$existingProvenance.source_commit -notmatch '^[0-9a-fA-F]{40}$' -or
        [string]$existingProvenance.source_tree -notmatch '^[0-9a-fA-F]{40}$' -or
        $existingInstallHeader -ne 'HLS Downloader 7.0.0') {
        throw "Existing E:\h directory is not an owned v7.0.0 installation: $target"
    }
}
if (Test-Path -LiteralPath $stage) {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
if (Test-Path -LiteralPath $backup) {
    throw "Previous local rollback image still exists: $backup"
}
if (Test-Path -LiteralPath $desktopExtensionStage) {
    Remove-Item -LiteralPath $desktopExtensionStage -Recurse -Force
}
if (Test-Path -LiteralPath $desktopExtensionBackup) {
    throw "Previous desktop extension rollback image still exists: $desktopExtensionBackup"
}

New-Item -ItemType Directory -Force -Path (Split-Path $target -Parent) | Out-Null
Copy-Item -LiteralPath $source -Destination $stage -Recurse -Force
Remove-Item -LiteralPath (Join-Path $stage 'portable') -Force -ErrorAction SilentlyContinue
# jpackage marks launchers and runtime files read-only. Normalize the staged
# image so a later transactional upgrade can test and replace it normally.
Get-ChildItem -LiteralPath $stage -Recurse -File -Force | ForEach-Object {
    if ($_.IsReadOnly) { $_.IsReadOnly = $false }
}
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'extensions') | Out-Null
foreach ($browser in @('Chromium', 'Firefox')) {
    Copy-Item -LiteralPath $extensionArchivePaths[$browser] `
        -Destination (Join-Path $stage "extensions\HLSDownloader-7.0.0-$browser.zip") -Force
}
Copy-Item -LiteralPath $note -Destination (Join-Path $stage 'HLS-Downloader-7.0.0-升级说明.md') -Force
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'scripts') | Out-Null
Copy-Item -LiteralPath (Join-Path $repo 'scripts\upgrade-v7-portable.ps1') -Destination (Join-Path $stage 'scripts\upgrade-v7-portable.ps1') -Force

$installInfo = @(
    'HLS Downloader 7.0.0',
    "InstalledAt=$([DateTimeOffset]::Now.ToString('o'))",
    "SourcePortableSHA256=$($portableHash.ToUpperInvariant())",
    "ArtifactManifestSHA256=$((Get-FileHash -LiteralPath $artifactManifest -Algorithm SHA256).Hash)",
    "ExecutableSHA256=$((Get-FileHash -LiteralPath (Join-Path $stage 'HLSDownloader.exe') -Algorithm SHA256).Hash)"
) -join "`r`n"
[IO.File]::WriteAllText((Join-Path $stage 'INSTALLATION.txt'), $installInfo, [Text.UTF8Encoding]::new($false))

$targetMovedToBackup = $false
$stageMovedToTarget = $false
$backupOwnerWritten = $false
try {
    if ($hadPrevious) {
        $shutdownScript = Join-Path $repo 'scripts\shutdown-running.ps1'
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $shutdownScript -InstallDir $target
        if ($LASTEXITCODE -ne 0) {
            throw "v7 running installation did not shut down cleanly (exit $LASTEXITCODE)"
        }
        Move-Item -LiteralPath $target -Destination $backup
        $targetMovedToBackup = $true
        $backupOwnerWritten = $true
        [IO.File]::WriteAllText((Join-Path $backup $ownerMarkerName),
            ([ordered]@{ schema = 1; target = $target; nonce = $transactionNonce } | ConvertTo-Json -Compress),
            [Text.UTF8Encoding]::new($false))
    }
    Move-Item -LiteralPath $stage -Destination $target
    $stageMovedToTarget = $true
} catch {
    if ($stageMovedToTarget -and (Test-Path -LiteralPath $target)) {
        Remove-Item -LiteralPath $target -Recurse -Force
    }
    if ($targetMovedToBackup -and (Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $target)) {
        Move-Item -LiteralPath $backup -Destination $target
        if ($backupOwnerWritten) {
            Remove-Item -LiteralPath (Join-Path $target $ownerMarkerName) -Force -ErrorAction SilentlyContinue
        }
    }
    throw
}

$engineExecutable = Join-Path $target 'app\resources\HLSDownloaderEngine.exe'
$hostExecutable = Join-Path $target 'app\resources\HLSDownloaderNativeHost.exe'
$desktop = [Environment]::GetFolderPath('Desktop')
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\HLS Downloader'
$desktopShortcut = Join-Path $desktop 'HLS Downloader 7.0.0.lnk'
$desktopExtensionPaths = @{
    Chromium = Join-Path $desktop 'HLSDownloader-Chromium.zip'
    Firefox = Join-Path $desktop 'HLSDownloader-Firefox.zip'
}
function Get-DesktopExtensionArchives([string]$Browser) {
    $browserPattern = if ($Browser -eq 'Chromium') { 'chromium|chrome' } else { 'firefox' }
    return @(Get-ChildItem -LiteralPath $desktop -Filter '*.zip' -File -ErrorAction SilentlyContinue |
        Where-Object {
            $_.Name -match ("^(?:HLS(?:[- _]?)Downloader|hls-downloader-extension)(?:[- _].*)?(?:" + $browserPattern + ")(?:[- _].*)?\.zip$")
        })
}
$installedDesktopExtensions = New-Object System.Collections.Generic.List[string]
try {
    $registration = Start-Process -FilePath $engineExecutable -ArgumentList '--register-native-host' -NoNewWindow -Wait -PassThru
    if ($registration.ExitCode -ne 0) {
        throw "v7 Native Host registration failed with exit $($registration.ExitCode)"
    }

    $shell = New-Object -ComObject WScript.Shell
    New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
    $shortcut = $shell.CreateShortcut((Join-Path $startMenu 'HLS Downloader 7.0.0.lnk'))
    $shortcut.TargetPath = Join-Path $target 'HLSDownloader.exe'
    $shortcut.WorkingDirectory = $target
    $shortcut.IconLocation = (Join-Path $target 'HLSDownloader.exe') + ',0'
    $shortcut.Description = 'HLS Downloader 7.0.0'
    $shortcut.Save()

    $desktopLink = $shell.CreateShortcut($desktopShortcut)
    $desktopLink.TargetPath = Join-Path $target 'HLSDownloader.exe'
    $desktopLink.WorkingDirectory = $target
    $desktopLink.IconLocation = (Join-Path $target 'HLSDownloader.exe') + ',0'
    $desktopLink.Description = 'HLS Downloader 7.0.0'
    $desktopLink.Save()

    New-Item -ItemType Directory -Force -Path $desktopExtensionStage | Out-Null
    foreach ($browser in @('Chromium', 'Firefox')) {
        Copy-Item -LiteralPath (Join-Path $target "extensions\HLSDownloader-7.0.0-$browser.zip") `
            -Destination (Join-Path $desktopExtensionStage "$browser.zip") -Force
    }
    New-Item -ItemType Directory -Force -Path $desktopExtensionBackup | Out-Null
    [IO.File]::WriteAllText((Join-Path $desktopExtensionBackup $ownerMarkerName),
        ([ordered]@{ schema = 1; target = $target; nonce = $transactionNonce } | ConvertTo-Json -Compress),
        [Text.UTF8Encoding]::new($false))
    # Keep exactly one current extension package per browser on the desktop.
    Get-ChildItem -LiteralPath $desktop -Filter 'HLS Downloader*浏览器插件*' -Directory -ErrorAction SilentlyContinue |
        Move-Item -Destination $desktopExtensionBackup -Force
    foreach ($browser in @('Chromium', 'Firefox')) {
        Get-DesktopExtensionArchives $browser | Move-Item -Destination $desktopExtensionBackup -Force
        Move-Item -LiteralPath (Join-Path $desktopExtensionStage "$browser.zip") `
            -Destination $desktopExtensionPaths[$browser] -Force
        [void]$installedDesktopExtensions.Add($desktopExtensionPaths[$browser])
        $published = @(Get-DesktopExtensionArchives $browser)
        if ($published.Count -ne 1 -or
            -not [String]::Equals($published[0].FullName, $desktopExtensionPaths[$browser], [StringComparison]::OrdinalIgnoreCase)) {
            throw "Desktop must contain exactly one current $browser extension archive."
        }
        $installedArchive = Join-Path $target "extensions\HLSDownloader-7.0.0-$browser.zip"
        if ((Get-FileHash -LiteralPath $published[0].FullName -Algorithm SHA256).Hash -ne
            (Get-FileHash -LiteralPath $installedArchive -Algorithm SHA256).Hash) {
            throw "Desktop $browser extension archive does not match the installed package."
        }
    }
    $installationPath = Join-Path $target 'INSTALLATION.txt'
    [IO.File]::WriteAllText($finalizeMarker,
        ([ordered]@{
            schema = 1
            version = '7.0.0'
            target = $target
            nonce = $transactionNonce
            root_backup_expected = $hadPrevious
            desktop_backup_expected = $true
            installation_sha256 = (Get-FileHash -LiteralPath $installationPath -Algorithm SHA256).Hash.ToLowerInvariant()
            provenance_sha256 = (Get-FileHash -LiteralPath (Join-Path $target 'app\resources\BUILD-PROVENANCE.json') -Algorithm SHA256).Hash.ToLowerInvariant()
        } | ConvertTo-Json -Depth 3),
        [Text.UTF8Encoding]::new($false))
} catch {
    $installError = $_
    $rollbackErrors = New-Object System.Collections.Generic.List[string]
    Remove-Item -LiteralPath $finalizeMarker -Force -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $desktopExtensionBackup) {
        $installedDesktopExtensions | ForEach-Object {
            Remove-Item -LiteralPath $_ -Force -ErrorAction SilentlyContinue
        }
        Remove-Item -LiteralPath (Join-Path $desktopExtensionBackup $ownerMarkerName) -Force -ErrorAction SilentlyContinue
        Get-ChildItem -LiteralPath $desktopExtensionBackup -Force -ErrorAction SilentlyContinue |
            Move-Item -Destination $desktop -Force
    }
    if (Test-Path -LiteralPath $desktopExtensionStage) {
        Remove-Item -LiteralPath $desktopExtensionStage -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $desktopExtensionBackup) {
        Remove-Item -LiteralPath $desktopExtensionBackup -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $target) {
        try {
            $unregister = Start-Process -FilePath $engineExecutable -ArgumentList '--unregister-native-host' -NoNewWindow -Wait -PassThru
            if ($unregister.ExitCode -ne 0) { throw "exit $($unregister.ExitCode)" }
        } catch {
            [void]$rollbackErrors.Add("unregister new Native Host: $($_.Exception.Message)")
        }
        Remove-Item -LiteralPath $target -Recurse -Force
    }
    if ($hadPrevious -and (Test-Path -LiteralPath $backup)) {
        Move-Item -LiteralPath $backup -Destination $target
        Remove-Item -LiteralPath (Join-Path $target $ownerMarkerName) -Force -ErrorAction SilentlyContinue
        try {
            $restoredEngine = Join-Path $target 'app\resources\HLSDownloaderEngine.exe'
            $restoreRegistration = Start-Process -FilePath $restoredEngine -ArgumentList '--register-native-host' -NoNewWindow -Wait -PassThru
            if ($restoreRegistration.ExitCode -ne 0) { throw "exit $($restoreRegistration.ExitCode)" }
        } catch {
            [void]$rollbackErrors.Add("restore previous Native Host: $($_.Exception.Message)")
        }
    } elseif (-not $hadPrevious) {
        Remove-Item -LiteralPath (Join-Path $startMenu 'HLS Downloader 7.0.0.lnk') -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $desktopShortcut -Force -ErrorAction SilentlyContinue
    }
    if ($rollbackErrors.Count -gt 0) {
        throw "$($installError.Exception.Message) Rollback errors: $($rollbackErrors -join '; ')"
    }
    throw $installError
}
if ($hadPrevious -and (Test-Path -LiteralPath $backup)) {
    Remove-OwnedDirectory $backup $target $transactionNonce
}
if (Test-Path -LiteralPath $desktopExtensionStage) {
    Remove-Item -LiteralPath $desktopExtensionStage -Recurse -Force
}
if (Test-Path -LiteralPath $desktopExtensionBackup) {
    Remove-OwnedDirectory $desktopExtensionBackup $target $transactionNonce
}
if (Test-Path -LiteralPath $finalizeMarker) {
    Remove-Item -LiteralPath $finalizeMarker -Force
}

[ordered]@{
    installed = $true
    version = '7.0.0'
    target = $target
    rollback = ''
    native_host = $hostExecutable
    chromium_extension = Join-Path $target 'extensions\HLSDownloader-7.0.0-Chromium.zip'
    firefox_extension = Join-Path $target 'extensions\HLSDownloader-7.0.0-Firefox.zip'
    desktop_chromium_extension = $desktopExtensionPaths.Chromium
    desktop_firefox_extension = $desktopExtensionPaths.Firefox
    desktop_extension_count = 2
    start_menu = Join-Path $startMenu 'HLS Downloader 7.0.0.lnk'
    desktop = $desktopShortcut
} | ConvertTo-Json -Depth 3
} finally {
    Remove-Item -LiteralPath $installSourceStage -Recurse -Force -ErrorAction SilentlyContinue
}
