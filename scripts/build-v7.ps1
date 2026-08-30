[CmdletBinding()]
param([ValidateSet('run','test','candidate','package','adversarial')][string]$Task='run')
$ErrorActionPreference = 'Stop'
$repo=(Resolve-Path "$PSScriptRoot\..").Path
$protocolSource = Get-Content -LiteralPath (Join-Path $repo 'desktop_ui\src\main\kotlin\com\hlsdownloader\desktop\Protocol.kt') -Raw -Encoding UTF8
if ($protocolSource -notmatch 'hls-downloader-v7-core' -or $protocolSource -notmatch 'HLSDownloader\.v7') {
    throw 'v7 build refused: Compose IPC defaults are not v7.'
}
$contractSource = Get-Content -LiteralPath (Join-Path $repo 'native_shell\src\contract.rs') -Raw -Encoding UTF8
if ($contractSource -notmatch 'V7_PROTOCOL_NAME') {
    throw 'v7 build refused: Rust v7 protocol contract is missing.'
}
$featureParity = Join-Path $repo 'artifacts\v7-productization\feature-parity.json'
$isPackage = @('candidate', 'package') -contains $Task
$packageTier = if ($Task -eq 'candidate') { 'candidate' } else { 'formal' }
$packageRoot = if ($Task -eq 'candidate') {
    Join-Path $repo 'artifacts\v7-productization\candidate'
} else {
    Join-Path $repo 'artifacts\v7-productization\package'
}
$provenance = Join-Path $packageRoot 'BUILD-PROVENANCE.json'
$artifactSuffix = if ($Task -eq 'candidate') { '-candidate' } else { '' }
$provenanceForBuild = $provenance
$candidateProvenanceTemp = $null
$candidateStagingRoot = $null
$candidateSwapBackupRoot = $null
if ($Task -eq 'candidate') {
    # Candidate packages are for external machine validation. They require the
    # canonical matrix, no blocked features and a clean tree, but not
    # release_ready or complete verification yet.
    # Write provenance outside the replaceable candidate directory first, so a
    # Failed gates never destroy the last candidate that passed validation.
    $candidateProvenanceTemp = Join-Path (Split-Path $packageRoot -Parent) ('.BUILD-PROVENANCE.candidate.' + [guid]::NewGuid().ToString('n') + '.tmp')
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$repo\scripts\verify-v7-feature-parity.ps1" -FeatureParityPath $featureParity -RequireNoBlocked -RequireCleanWorktree -PackageTier candidate -ProvenancePath $candidateProvenanceTemp
    if ($LASTEXITCODE -ne 0) {
        Remove-Item -LiteralPath $candidateProvenanceTemp -Force -ErrorAction SilentlyContinue
        exit $LASTEXITCODE
    }
}
if ($Task -eq 'package') {
    # Formal packages add the release_ready decision after candidate validation.
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$repo\scripts\verify-v7-feature-parity.ps1" -FeatureParityPath $featureParity -RequireCanonicalComplete -RequireReleaseReady -RequireCleanWorktree -PackageTier formal -ProvenancePath $provenance
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
try {
$artifactRoot = $packageRoot
if ($Task -eq 'candidate') {
    $candidateStagingRoot = Join-Path (Split-Path $packageRoot -Parent) ('.candidate-staging.' + [guid]::NewGuid().ToString('n'))
    New-Item -ItemType Directory -Force -Path $candidateStagingRoot | Out-Null
    $provenanceForBuild = Join-Path $candidateStagingRoot 'BUILD-PROVENANCE.json'
    Move-Item -LiteralPath $candidateProvenanceTemp -Destination $provenanceForBuild -Force
    $artifactRoot = $candidateStagingRoot
}
$env:CARGO_HOME='E:\HLSDownloaderBuildCache\cargo'
$env:CARGO_TARGET_DIR='D:\HLSDownloaderBuildCache\cargo-target'
$env:GRADLE_USER_HOME='E:\HLSDownloaderBuildCache\gradle'
$env:JAVA_HOME='E:\HLSDownloaderBuildCache\jdk-21'
if(!(Test-Path "$env:JAVA_HOME\bin\java.exe")){ throw "JDK 21 is missing at $env:JAVA_HOME. Run scripts\bootstrap-v7-toolchain.ps1." }
$libMpvCache = 'E:\HLSDownloaderBuildCache\libmpv-20260814'
$sevenZipExe = Join-Path $libMpvCache '7zr.exe'
$libMpvArchive = Join-Path $libMpvCache 'mpv-dev-x86_64.7z'
$sevenZipUrl = 'https://github.com/ip7z/7zip/releases/download/26.02/7zr.exe'
$sevenZipSha256 = '56b8cc9f4971cef253644fafe54063ed7fdca551d4dee0f8c6baa81b855acd72'
$libMpvArchiveUrl = 'https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20260814/mpv-dev-x86_64-20260814-git-7b8915bc1d.7z'
$libMpvArchiveSha256 = '0af22b28e920620036d3ae08fd9283156dc9af0420bf4df84b0e02282094599c'
$curlImpersonateVersion = 'v2.0.0'
$curlImpersonateCache = "E:\HLSDownloaderBuildCache\curl-impersonate-$curlImpersonateVersion"
$curlImpersonateArchive = Join-Path $curlImpersonateCache 'curl-impersonate-v2.0.0.x86_64-win32.tar.gz'
$curlImpersonateUrl = 'https://github.com/lexiforest/curl-impersonate/releases/download/v2.0.0/curl-impersonate-v2.0.0.x86_64-win32.tar.gz'
$curlImpersonateSha256 = 'd2e5905f8adf76f042afe78d1758a978253afddf4eb7bdcb8ddfb38c2f0e530c'

function Assert-FileSha256([string]$Path, [string]$Expected, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path)) { throw "$Label is missing: $Path" }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $Expected.ToLowerInvariant()) { throw "$Label SHA-256 mismatch: expected $Expected, got $actual" }
}

function Get-VerifiedFile([string]$Url, [string]$Path, [string]$Expected, [string]$Label) {
    New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($Path)) | Out-Null
    if (Test-Path -LiteralPath $Path) {
        try { Assert-FileSha256 $Path $Expected $Label; return } catch { Remove-Item -LiteralPath $Path -Force }
    }
    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
    if ($curl) {
        & $curl.Source --location --fail --retry 3 --retry-delay 2 --max-time 900 --output $Path $Url
        if ($LASTEXITCODE -ne 0) { throw "$Label download failed with exit $LASTEXITCODE" }
    } else {
        Invoke-WebRequest -Uri $Url -OutFile $Path -MaximumRedirection 10
    }
    Assert-FileSha256 $Path $Expected $Label
}

function Copy-LibMpv([string]$Destination) {
    Get-VerifiedFile $sevenZipUrl $sevenZipExe $sevenZipSha256 '7zr.exe'
    Get-VerifiedFile $libMpvArchiveUrl $libMpvArchive $libMpvArchiveSha256 'libmpv archive'
    $extract = Join-Path $libMpvCache 'extract'
    if (Test-Path -LiteralPath $extract) { Remove-Item -LiteralPath $extract -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $extract | Out-Null
    try {
        & $sevenZipExe e -y "-o$extract" $libMpvArchive 'libmpv-2.dll'
        if ($LASTEXITCODE -ne 0) { throw "7zr failed to extract libmpv-2.dll (exit $LASTEXITCODE)" }
        $dll = Get-ChildItem -LiteralPath $extract -Filter 'libmpv-2.dll' -Recurse -File | Select-Object -First 1
        if (-not $dll) { throw 'libmpv archive did not contain libmpv-2.dll' }
        Copy-Item -LiteralPath $dll.FullName -Destination (Join-Path $Destination 'libmpv-2.dll') -Force
        Assert-FileSha256 (Join-Path $Destination 'libmpv-2.dll') ((Get-FileHash -LiteralPath $dll.FullName -Algorithm SHA256).Hash) 'bundled libmpv-2.dll'
    } finally { Remove-Item -LiteralPath $extract -Recurse -Force -ErrorAction SilentlyContinue }
}

function Copy-CurlImpersonate([string]$Destination) {
    Get-VerifiedFile $curlImpersonateUrl $curlImpersonateArchive $curlImpersonateSha256 "curl-impersonate $curlImpersonateVersion Windows x64 archive"
    $systemTar = Join-Path $env:SystemRoot 'System32\tar.exe'
    $tarExe = if (Test-Path -LiteralPath $systemTar -PathType Leaf) {
        $systemTar
    } else {
        $tarCommand = Get-Command tar.exe -ErrorAction SilentlyContinue
        if ($tarCommand) { $tarCommand.Source }
    }
    if ([String]::IsNullOrWhiteSpace($tarExe)) { throw 'tar.exe is required to extract curl-impersonate.' }
    $extract = Join-Path $curlImpersonateCache 'extract'
    if (Test-Path -LiteralPath $extract) { Remove-Item -LiteralPath $extract -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $extract | Out-Null
    try {
        & $tarExe -xzf $curlImpersonateArchive -C $extract './curl-impersonate.exe'
        if ($LASTEXITCODE -ne 0) { throw "tar.exe failed to extract curl-impersonate.exe (exit $LASTEXITCODE)" }
        $source = Join-Path $extract 'curl-impersonate.exe'
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            throw 'Verified curl-impersonate archive did not contain curl-impersonate.exe.'
        }
        $toolDirectory = Join-Path (Join-Path $Destination 'tools') 'curl-impersonate'
        New-Item -ItemType Directory -Force -Path $toolDirectory | Out-Null
        $target = Join-Path $toolDirectory 'curl-impersonate.exe'
        Copy-Item -LiteralPath $source -Destination $target -Force
        Assert-FileSha256 $target ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash) 'bundled curl-impersonate.exe'
    } finally {
        Remove-Item -LiteralPath $extract -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Build-Extension([string]$Resources) {
    $pnpm = Get-Command pnpm.cmd -ErrorAction SilentlyContinue
    if (-not $pnpm) { throw 'pnpm.cmd is required to build the production browser extension.' }
    $pnpmVersion = (& $pnpm.Source --version).Trim()
    if ($LASTEXITCODE -ne 0 -or $pnpmVersion -ne '11.7.0') {
        throw "Production extension build requires pnpm 11.7.0; found $pnpmVersion"
    }
    $package = Get-Content -LiteralPath (Join-Path $repo 'extension\package.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($package.version -ne '7.0.0') { throw "Browser extension package version must be 7.0.0: $($package.version)" }
    Push-Location (Join-Path $repo 'extension')
    try {
        & $pnpm.Source install --frozen-lockfile
        if ($LASTEXITCODE -ne 0) { throw "pnpm install failed with exit $LASTEXITCODE" }
        & $pnpm.Source run build
        if ($LASTEXITCODE -ne 0) { throw "pnpm run build failed with exit $LASTEXITCODE" }
    } finally { Pop-Location }
    New-Item -ItemType Directory -Force -Path (Join-Path $Resources 'extensions') | Out-Null
    foreach ($item in @(
        @{ Source = 'chrome-mv3'; Name = 'HLSDownloader-7.0.0-Chromium.zip' },
        @{ Source = 'firefox-mv3'; Name = 'HLSDownloader-7.0.0-Firefox.zip' }
    )) {
        $source = Join-Path (Join-Path $repo 'extension\.output') $item.Source
        if (-not (Test-Path -LiteralPath $source -PathType Container)) { throw "Production extension output is missing: $source" }
        $manifest = Get-Content -LiteralPath (Join-Path $source 'manifest.json') -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($manifest.version -ne '7.0.0') { throw "Built $($item.Source) manifest version is not 7.0.0: $($manifest.version)" }
        Compress-Archive -Path (Join-Path $source '*') -DestinationPath (Join-Path (Join-Path $Resources 'extensions') $item.Name) -CompressionLevel Optimal -Force
    }
}

if ($isPackage) {
    Build-Extension (Join-Path $repo 'desktop_ui\resources\common')
}

$cargo = 'C:\Users\lee\.cargo\bin\cargo.exe'
$engineTarget = if ($isPackage) { 'release' } else { 'debug' }
& $cargo build --manifest-path "$repo\native_shell\Cargo.toml" $(if ($engineTarget -eq 'release') { '--release' }) --bin hls-downloader-engine
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $cargo build --manifest-path "$repo\native_shell\Cargo.toml" $(if ($engineTarget -eq 'release') { '--release' }) --bin HLSDownloaderNativeHost
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $cargo build --manifest-path "$repo\native_shell\Cargo.toml" $(if ($engineTarget -eq 'release') { '--release' }) --bin HLSDownloaderUpdater
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $cargo build --manifest-path "$repo\presenter_ui\Cargo.toml" $(if ($engineTarget -eq 'release') { '--release' }) --bin hls-downloader-presenter
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$engine = Join-Path $env:CARGO_TARGET_DIR "$engineTarget\hls-downloader-engine.exe"
if (!(Test-Path -LiteralPath $engine)) { throw "Rust engine was not produced: $engine" }
$nativeHost = Join-Path $env:CARGO_TARGET_DIR "$engineTarget\HLSDownloaderNativeHost.exe"
if (!(Test-Path -LiteralPath $nativeHost)) { throw "Native Messaging host was not produced: $nativeHost" }
$updater = Join-Path $env:CARGO_TARGET_DIR "$engineTarget\HLSDownloaderUpdater.exe"
if (!(Test-Path -LiteralPath $updater)) { throw "Update helper was not produced: $updater" }
$presenter = Join-Path $env:CARGO_TARGET_DIR "$engineTarget\hls-downloader-presenter.exe"
if (!(Test-Path -LiteralPath $presenter)) { throw "v7 presenter was not produced: $presenter" }
Push-Location "$repo\desktop_ui"
try {
if ($isPackage) {
    $resources = Join-Path $repo 'desktop_ui\resources\common'
    New-Item -ItemType Directory -Force -Path $resources | Out-Null
    Copy-Item -LiteralPath (Join-Path $repo 'assets\app-icon.ico') -Destination (Join-Path $resources 'app-icon.ico') -Force
    Copy-Item -LiteralPath $engine -Destination (Join-Path $resources 'HLSDownloaderEngine.exe') -Force
    # The dedicated bridge has no Compose/Slint dependency and never opens SQLite.
    Copy-Item -LiteralPath $nativeHost -Destination (Join-Path $resources 'HLSDownloaderNativeHost.exe') -Force
    Copy-Item -LiteralPath $updater -Destination (Join-Path $resources 'HLSDownloaderUpdater.exe') -Force
    Copy-Item -LiteralPath $presenter -Destination (Join-Path $resources 'HLSDownloaderPresenter.exe') -Force
    Copy-Item -LiteralPath $featureParity -Destination (Join-Path $resources 'FEATURE-PARITY.json') -Force
    Copy-Item -LiteralPath $provenanceForBuild -Destination (Join-Path $resources 'BUILD-PROVENANCE.json') -Force
    # Ship the media tools beside the v7 workbench when the local toolchain
    # provides them. The Core reads these names from its packaged directory.
    $ffmpegRoot = 'C:\Users\lee\.conda\envs\test\Library\bin'
    foreach ($tool in @('ffmpeg.exe', 'ffprobe.exe', 'ffplay.exe')) {
        $source = Join-Path $ffmpegRoot $tool
        if (Test-Path -LiteralPath $source) {
            Copy-Item -LiteralPath $source -Destination (Join-Path $resources $tool) -Force
        }
    }
    Copy-CurlImpersonate $resources
    Copy-LibMpv $resources
    # Compose's jlink task rejects a leftover output directory after an interrupted package run.
    $runtimeImage = 'D:\HLSDownloaderBuildCache\compose-build\compose\tmp\main\runtime'
    if (Test-Path -LiteralPath $runtimeImage) {
        Remove-Item -LiteralPath $runtimeImage -Recurse -Force
    }
}
$env:HLS_ENGINE_PATH = $engine
    switch ($Task) {
        'run' {
            $engineProcess = Start-Process -FilePath $engine -WorkingDirectory (Split-Path $engine -Parent) -PassThru
            Start-Sleep -Milliseconds 250
            $presenterProcess = Start-Process -FilePath $presenter -WorkingDirectory (Split-Path $presenter -Parent) -PassThru
            try { & .\gradlew.bat run } finally {
                if ($presenterProcess -and -not $presenterProcess.HasExited) { $presenterProcess.CloseMainWindow() | Out-Null }
            }
        }
        'test' { & .\gradlew.bat test }
    'candidate' { & .\gradlew.bat clean createDistributable packageDistributionForCurrentOS }
        'package' { & .\gradlew.bat clean createDistributable packageDistributionForCurrentOS }
        'adversarial' { & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$repo\scripts\adversarial-v7.ps1" -Scope native }
    }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($isPackage) {
        $exe = Get-ChildItem -LiteralPath 'D:\HLSDownloaderBuildCache\compose-build\compose\binaries\main\exe' -Filter 'HLSDownloader-7.0.0.exe' -File -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $exe) { throw 'The v7 installer EXE was not produced in the isolated build cache.' }
        $msi = Get-ChildItem -LiteralPath 'D:\HLSDownloaderBuildCache\compose-build\compose\binaries\main\msi' -Filter 'HLSDownloader-7.0.0.msi' -File -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $msi) { throw 'The v7 MSI was not produced in the isolated build cache.' }
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$repo\scripts\set-v7-msi-rollback-order.ps1" -MsiPath $msi.FullName
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null
        Copy-Item -LiteralPath $exe.FullName -Destination (Join-Path $artifactRoot ("HLSDownloader-7.0.0-Windows-x64$artifactSuffix.exe")) -Force
        Copy-Item -LiteralPath $msi.FullName -Destination (Join-Path $artifactRoot ("HLSDownloader-7.0.0-Windows-x64$artifactSuffix.msi")) -Force
        $portablePath = Join-Path $artifactRoot ("HLSDownloader-7.0.0-Windows-x64-Portable$artifactSuffix.zip")
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$repo\scripts\create-v7-portable.ps1" -OutZip $portablePath
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        $portableCheck = Join-Path $artifactRoot ('.portable-verify-' + [guid]::NewGuid().ToString('n'))
        try {
            Expand-Archive -LiteralPath $portablePath -DestinationPath $portableCheck -Force
            $portableRoot = Join-Path $portableCheck 'HLSDownloader'
            $provenanceInPackage = Join-Path $portableRoot 'app\resources\BUILD-PROVENANCE.json'
            if (-not (Test-Path -LiteralPath $provenanceInPackage -PathType Leaf)) {
                throw 'Portable package is missing app/resources/BUILD-PROVENANCE.json.'
            }
            $provenanceJson = Get-Content -LiteralPath $provenanceInPackage -Raw -Encoding UTF8 | ConvertFrom-Json
            $sourceCommit = (& git -C $repo rev-parse HEAD).Trim()
            $sourceTree = (& git -C $repo rev-parse HEAD^{tree}).Trim()
            if ($provenanceJson.source_commit -ne $sourceCommit) {
                throw "Portable provenance source_commit does not match HEAD: $($provenanceJson.source_commit) != $sourceCommit"
            }
            if ($provenanceJson.source_tree -ne $sourceTree) {
                throw "Portable provenance source_tree does not match HEAD: $($provenanceJson.source_tree) != $sourceTree"
            }
            if ($provenanceJson.package_tier -ne $packageTier) {
                throw "Portable provenance package_tier does not match the build tier: $($provenanceJson.package_tier) != $packageTier"
            }
            if ($provenanceJson.product_version -ne '7.0.0') {
                throw "Portable provenance product_version is not 7.0.0: $($provenanceJson.product_version)"
            }
            $featureInPackage = Join-Path $portableRoot 'app\resources\FEATURE-PARITY.json'
            if (-not (Test-Path -LiteralPath $featureInPackage -PathType Leaf)) {
                throw 'Portable package is missing app/resources/FEATURE-PARITY.json.'
            }
            $featureHashInPackage = (Get-FileHash -LiteralPath $featureInPackage -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($provenanceJson.feature_parity_sha256 -ne $featureHashInPackage) {
                throw "Portable feature parity hash does not match provenance: $($provenanceJson.feature_parity_sha256) != $featureHashInPackage"
            }
            foreach ($extension in @('Chromium', 'Firefox')) {
                $archive = Join-Path $portableRoot ("extensions\HLSDownloader-7.0.0-$extension.zip")
                if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
                    throw "Portable package is missing the $extension extension archive."
                }
                $extensionCheck = Join-Path $portableCheck ("extension-$extension")
                Expand-Archive -LiteralPath $archive -DestinationPath $extensionCheck -Force
                $manifestPath = Join-Path $extensionCheck 'manifest.json'
                if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
                    throw "$extension extension archive is missing manifest.json."
                }
                $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
                if ($manifest.version -ne '7.0.0') {
                    throw "$extension extension manifest version is not 7.0.0: $($manifest.version)"
                }
            }
        } finally {
            Remove-Item -LiteralPath $portableCheck -Recurse -Force -ErrorAction SilentlyContinue
        }
        Copy-Item -LiteralPath $featureParity -Destination (Join-Path $artifactRoot 'FEATURE-PARITY.json') -Force
        if ($Task -eq 'candidate') {
            # Swap the complete staging directory only after every artifact is ready.
            $candidateSwapBackupRoot = Join-Path (Split-Path $packageRoot -Parent) ('.candidate-backup.' + [guid]::NewGuid().ToString('n'))
            $oldCandidateMoved = $false
            try {
                if (Test-Path -LiteralPath $packageRoot) {
                    Move-Item -LiteralPath $packageRoot -Destination $candidateSwapBackupRoot
                    $oldCandidateMoved = $true
                }
                Move-Item -LiteralPath $candidateStagingRoot -Destination $packageRoot
                $candidateStagingRoot = $null

                if ($oldCandidateMoved -and (Test-Path -LiteralPath $candidateSwapBackupRoot)) {
                    try {
                        Remove-Item -LiteralPath $candidateSwapBackupRoot -Recurse -Force -ErrorAction Stop
                        $candidateSwapBackupRoot = $null
                    } catch {
                        Write-Warning "The new candidate is installed, but the previous candidate backup could not be removed: $candidateSwapBackupRoot"
                        # The new candidate is already committed. Preserve both directories.
                        $candidateSwapBackupRoot = $null
                    }
                }
            } catch {
                if ($oldCandidateMoved -and (Test-Path -LiteralPath $packageRoot)) {
                    Remove-Item -LiteralPath $packageRoot -Recurse -Force -ErrorAction SilentlyContinue
                }
                if ($oldCandidateMoved -and (Test-Path -LiteralPath $candidateSwapBackupRoot)) {
                    Move-Item -LiteralPath $candidateSwapBackupRoot -Destination $packageRoot -Force
                    $candidateSwapBackupRoot = $null
                }
                throw
            }
        }
    }
} finally {
    Pop-Location
}
} finally {
    if ($null -ne $candidateProvenanceTemp -and (Test-Path -LiteralPath $candidateProvenanceTemp)) {
        Remove-Item -LiteralPath $candidateProvenanceTemp -Force -ErrorAction SilentlyContinue
    }
    if ($null -ne $candidateStagingRoot -and (Test-Path -LiteralPath $candidateStagingRoot)) {
        Remove-Item -LiteralPath $candidateStagingRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
    # A swap failure may leave the last usable candidate at the backup path.
    # Preserve it for manual recovery when the in-place restoration also fails.
}
