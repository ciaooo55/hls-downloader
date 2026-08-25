[CmdletBinding()]
param([ValidateSet('run','test','package','adversarial')][string]$Task='run')
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
$provenance = Join-Path $repo 'artifacts\v7-productization\package\BUILD-PROVENANCE.json'
if ($Task -eq 'package') {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$repo\scripts\verify-v7-feature-parity.ps1" -FeatureParityPath $featureParity -RequireReleaseReady -RequireCleanWorktree -ProvenancePath $provenance
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
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
    $tar = Get-Command tar.exe -ErrorAction SilentlyContinue
    if (-not $tar) { throw 'tar.exe is required to extract curl-impersonate.' }
    $extract = Join-Path $curlImpersonateCache 'extract'
    if (Test-Path -LiteralPath $extract) { Remove-Item -LiteralPath $extract -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $extract | Out-Null
    try {
        & $tar.Source -xzf $curlImpersonateArchive -C $extract './curl-impersonate.exe'
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
$cargo = 'C:\Users\lee\.cargo\bin\cargo.exe'
$engineTarget = if ($Task -eq 'package') { 'release' } else { 'debug' }
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
if ($Task -eq 'package') {
    $resources = Join-Path $repo 'desktop_ui\resources\common'
    New-Item -ItemType Directory -Force -Path $resources | Out-Null
    Copy-Item -LiteralPath (Join-Path $repo 'assets\app-icon.ico') -Destination (Join-Path $resources 'app-icon.ico') -Force
    Copy-Item -LiteralPath $engine -Destination (Join-Path $resources 'HLSDownloaderEngine.exe') -Force
    # The dedicated bridge has no Compose/Slint dependency and never opens SQLite.
    Copy-Item -LiteralPath $nativeHost -Destination (Join-Path $resources 'HLSDownloaderNativeHost.exe') -Force
    Copy-Item -LiteralPath $updater -Destination (Join-Path $resources 'HLSDownloaderUpdater.exe') -Force
    Copy-Item -LiteralPath $presenter -Destination (Join-Path $resources 'HLSDownloaderPresenter.exe') -Force
    Copy-Item -LiteralPath $featureParity -Destination (Join-Path $resources 'FEATURE-PARITY.json') -Force
    Copy-Item -LiteralPath $provenance -Destination (Join-Path $resources 'BUILD-PROVENANCE.json') -Force
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
Push-Location "$repo\desktop_ui"
try {
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
    'package' { & .\gradlew.bat clean createDistributable packageDistributionForCurrentOS }
        'adversarial' { & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$repo\scripts\adversarial-v7.ps1" -Scope native }
    }
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($Task -eq 'package') {
        $msi = Get-ChildItem -LiteralPath 'D:\HLSDownloaderBuildCache\compose-build\compose\binaries\main\msi' -Filter 'HLSDownloader-7.0.0.msi' -File -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $msi) { throw 'The v7 MSI was not produced in the isolated build cache.' }
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$repo\scripts\set-v7-msi-rollback-order.ps1" -MsiPath $msi.FullName
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$repo\scripts\create-v7-portable.ps1" -OutZip "$repo\artifacts\v7-productization\package\HLSDownloader-7.0.0-Windows-x64-Portable.zip"
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        Copy-Item -LiteralPath $featureParity -Destination "$repo\artifacts\v7-productization\package\FEATURE-PARITY.json" -Force
    }
} finally { Pop-Location }
