param(
    [switch]$SkipFrontend,
    [switch]$SkipBackend,
    [switch]$SkipDesktop,
    [switch]$SkipInstaller,
    [switch]$SkipSmoke,
    [switch]$UseSystemFfmpeg,
    [switch]$IncludeExtensionAssets,
    [string]$Version = "5.0.3"
)

$ErrorActionPreference = "Stop"

function Write-Utf8NoBom([string]$Path, [string]$Value) {
    # `Set-Content -Encoding UTF8` writes a BOM in Windows PowerShell 5.1 but
    # not in PowerShell 7. Keep generated JSON/source artifacts identical in
    # both shells and accepted by strict UTF-8 readers.
    [System.IO.File]::WriteAllText(
        $Path,
        $Value,
        (New-Object System.Text.UTF8Encoding($false))
    )
}

function Get-DeclaredVersion([string]$Path, [string]$Pattern) {
    # Windows PowerShell 5.1 treats BOM-less UTF-8 as the active ANSI code
    # page. Every project manifest is UTF-8 and may contain Chinese text, so
    # make the encoding explicit instead of relying on the shell version.
    $content = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
    $match = [regex]::Match($content, $Pattern)
    if (-not $match.Success) {
        throw "Unable to read the declared version from $Path"
    }
    return $match.Groups[1].Value
}

$versionParts = @($Version -split '\.')
$invalidVersionPart = @($versionParts | Where-Object { $_ -notmatch '^\d+$' }).Count -gt 0
if ($versionParts.Count -gt 4 -or $versionParts.Count -lt 1 -or $invalidVersionPart) {
    throw "Version must contain one to four numeric parts: $Version"
}
while ($versionParts.Count -lt 4) { $versionParts += "0" }
$FileVersion = ($versionParts | ForEach-Object { [int]$_ }) -join "."

$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$declaredAppVersions = [ordered]@{
    "backend/app/version.py" = Get-DeclaredVersion (Join-Path $Root "backend\app\version.py") 'APP_VERSION\s*=\s*"([^"]+)"'
    "frontend/package.json" = (Get-Content -LiteralPath (Join-Path $Root "frontend\package.json") -Raw -Encoding UTF8 | ConvertFrom-Json).version
    "frontend/src-tauri/tauri.conf.json" = (Get-Content -LiteralPath (Join-Path $Root "frontend\src-tauri\tauri.conf.json") -Raw -Encoding UTF8 | ConvertFrom-Json).version
    "frontend/src-tauri/Cargo.toml" = Get-DeclaredVersion (Join-Path $Root "frontend\src-tauri\Cargo.toml") '(?m)^version\s*=\s*"([^"]+)"'
    "installer/hls-downloader.nsi" = Get-DeclaredVersion (Join-Path $Root "installer\hls-downloader.nsi") '!define APP_VERSION\s+"([^"]+)"'
}
foreach ($entry in $declaredAppVersions.GetEnumerator()) {
    if ($entry.Value -ne $Version) {
        throw "Release version $Version does not match $($entry.Key): $($entry.Value)"
    }
}
$FrontendDir = Join-Path $Root "frontend"
$BackendDir = Join-Path $Root "backend"
$ExtensionDir = Join-Path $Root "extension"
$AssetsDir = Join-Path $Root "assets"
$IconFile = Join-Path $AssetsDir "app-icon.ico"
$StageDir = Join-Path $Root "build\installer\stage"
$PortableStage = Join-Path $Root "build\installer\portable"
$PyInstallerVersionFile = Join-Path $Root "build\installer\pyinstaller-version.txt"
$ReleaseDir = Join-Path $Root "release"
$ToolsDir = Join-Path $Root "tools"
$BinDir = Join-Path $Root "bin"
$FFmpegArchive = Join-Path $ToolsDir "ffmpeg-windows.zip"
$FFmpegToolsDir = Join-Path $ToolsDir "ffmpeg-windows"
$FFmpegArchiveUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-01-13-21/ffmpeg-N-125881-g946272b79a-win64-gpl.zip"
$FFmpegArchiveBuild = "BtbN autobuild 2026-08-01 13:21 (FFmpeg g946272b79a)"
$FFmpegArchiveSha256 = "a082da6d5ce0cbb9a8ad0112ab7f654d480c707b8caf9d332f4532d78b65257f"
$NsisVersion = "3.12"
$NsisZip = Join-Path $ToolsDir "nsis-$NsisVersion.zip"
# makensis 3.x still resolves its built-in Stubs directory through an ANSI
# path on Windows.  Keeping the executable under a Chinese project path makes
# even an otherwise Unicode installer fail before parsing the script.  Extract
# the verified tool into a per-user ASCII-safe runtime directory instead.
$NsisRuntimeRoot = if ($env:LOCALAPPDATA) {
    Join-Path $env:LOCALAPPDATA "HLSDownloaderBuildTools"
} elseif ($env:TEMP) {
    Join-Path $env:TEMP "HLSDownloaderBuildTools"
} else {
    Join-Path ([IO.Path]::GetTempPath()) "HLSDownloaderBuildTools"
}
$NsisToolsDir = Join-Path $NsisRuntimeRoot "nsis-$NsisVersion"
$NsisUrl = "https://master.dl.sourceforge.net/project/nsis/NSIS%203/$NsisVersion/nsis-$NsisVersion.zip?viasf=1"
$NsisSha256 = "56581f90db321581c5381193d796fffcf2d24b2f8fed2160a6c6a3baa67f2c4f"
$InstallerScript = Join-Path $Root "installer\hls-downloader.nsi"
$ReleaseNamePrefix = "HLSDownloader-v$Version"
$InstallerOut = Join-Path $ReleaseDir "$ReleaseNamePrefix-Windows-x64-Setup.exe"
$PreservedInstaller = Join-Path $Root "build\installer\$ReleaseNamePrefix-Windows-x64-Setup.exe"
$PortableOut = Join-Path $ReleaseDir "$ReleaseNamePrefix-Windows-x64-Portable.zip"
$FirefoxId = "hls-downloader-store@ciaooo55.com"
$ExtensionBuildDir = Join-Path $Root "build\installer\extensions"
$FirefoxStage = Join-Path $ExtensionBuildDir "firefox"
$FirefoxExtensionOut = Join-Path $ReleaseDir "$ReleaseNamePrefix-Firefox-Unsigned.zip"
$FirefoxSourceOut = Join-Path $ReleaseDir "$ReleaseNamePrefix-Firefox-Source.zip"
$ChromiumExtensionStage = Join-Path $ExtensionBuildDir "chrome-edge"
$ChromiumExtensionOut = Join-Path $ReleaseDir "$ReleaseNamePrefix-Chrome-Edge-Extension.zip"

if ($IncludeExtensionAssets) {
    $declaredExtensionVersion = (Get-Content -LiteralPath (Join-Path $ExtensionDir "package.json") -Raw -Encoding UTF8 | ConvertFrom-Json).version
    $recommendedExtensionVersion = Get-DeclaredVersion (Join-Path $Root "backend\app\browser_handoff.py") 'RECOMMENDED_BROWSER_EXTENSION_VERSION\s*=\s*"([^"]+)"'
    if ($declaredExtensionVersion -ne $recommendedExtensionVersion) {
        throw "Extension assets require package and recommended versions to match each other"
    }
}

function Invoke-Step($Name, [scriptblock]$Block) {
    Write-Host ""
    Write-Host "==> $Name" -ForegroundColor Cyan
    & $Block
}

function Assert-FileSha256([string]$Path, [string]$Expected, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path)) {
        throw "$Label is missing: $Path"
    }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $Expected.ToLowerInvariant()) {
        throw "$Label SHA-256 mismatch. Expected $Expected, got $actual"
    }
}

function Get-VerifiedArchive([string]$Url, [string]$Path, [string]$Expected, [string]$Label) {
    New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($Path)) | Out-Null
    if (Test-Path -LiteralPath $Path) {
        try {
            Assert-FileSha256 $Path $Expected $Label
            return
        } catch {
            [System.IO.File]::Delete($Path)
        }
    }
    $lastError = $null
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            Write-Host "Downloading pinned $Label (attempt $attempt/3)..."
            # Windows PowerShell's Invoke-WebRequest can fail with
            # "Operation is not valid due to the current state of the
            # object" on SourceForge's redirect chain.  Prefer the native
            # curl.exe available on supported Windows 10/11 installations,
            # while retaining an Invoke-WebRequest fallback for minimal
            # environments without curl.
            $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
            if ($curl) {
                & $curl.Source --location --fail --retry 3 --retry-delay 2 --max-time 300 --output $Path $Url
                if ($LASTEXITCODE -ne 0) {
                    throw "curl.exe failed with exit code $LASTEXITCODE"
                }
            } else {
                Invoke-WebRequest -Uri $Url -OutFile $Path -MaximumRedirection 10
            }
            Assert-FileSha256 $Path $Expected $Label
            return
        } catch {
            $lastError = $_
            [System.IO.File]::Delete($Path)
            if ($attempt -lt 3) { Start-Sleep -Seconds (2 * $attempt) }
        }
    }
    throw "$Label download or verification failed: $($lastError.Exception.Message)"
}

function Invoke-PyInstallerWithRetry(
    [string]$Name,
    [scriptblock]$Build
) {
    # On some Windows Python environments pywin32-ctypes can fail during
    # PyInstaller's *process startup* immediately after another PyInstaller
    # build, even though importing it in a fresh process succeeds. Retrying a
    # clean child process is safe: --noconfirm/--clean make every attempt a
    # complete replacement and we never stage output until it succeeds.
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        & $Build
        if ($LASTEXITCODE -eq 0) { return }
        if ($attempt -lt 3) {
            Write-Warning "$Name PyInstaller startup/build failed (attempt $attempt/3); retrying in a fresh process..."
            Start-Sleep -Seconds (2 * $attempt)
        }
    }
    throw "$Name PyInstaller build failed with exit code $LASTEXITCODE"
}

function Get-MakeNsis {
    Get-VerifiedArchive $NsisUrl $NsisZip $NsisSha256 "NSIS $NsisVersion archive"
    if (-not (Test-Path -LiteralPath $NsisToolsDir)) {
        Expand-Archive -LiteralPath $NsisZip -DestinationPath $NsisToolsDir -Force
    }
    $makensis = Get-ChildItem -LiteralPath $NsisToolsDir -Recurse -File -Filter "makensis.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $makensis) {
        throw "Pinned NSIS archive did not contain makensis.exe"
    }
    return $makensis.FullName
}

function Find-MediaTool($Name) {
    if ($UseSystemFfmpeg) {
        # This opt-in is for a local build when the pinned archive mirror is
        # unavailable. CI and normal releases keep the verified archive path
        # below, so a developer machine can never silently change release
        # provenance. Copy-MediaTool still executes the binary to validate it.
        $systemTool = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if (-not $systemTool -or -not (Test-Path -LiteralPath $systemTool.Source)) {
            throw "-UseSystemFfmpeg requires $Name to be available on PATH"
        }
        return [IO.Path]::GetFullPath($systemTool.Source)
    }
    Get-VerifiedArchive $FFmpegArchiveUrl $FFmpegArchive $FFmpegArchiveSha256 "FFmpeg archive ($FFmpegArchiveBuild)"
    if (-not (Test-Path -LiteralPath $FFmpegToolsDir)) {
        Expand-Archive -LiteralPath $FFmpegArchive -DestinationPath $FFmpegToolsDir -Force
    }
    $downloaded = Get-ChildItem -LiteralPath $FFmpegToolsDir -Recurse -File -Filter $Name -ErrorAction SilentlyContinue |
        Sort-Object Length -Descending |
        Select-Object -First 1
    if (-not $downloaded) {
        throw "$Name was not found in the downloaded FFmpeg archive."
    }
    return $downloaded.FullName
}

function Copy-MediaTool($Name) {
    $destination = Join-Path $BinDir $Name
    $source = Find-MediaTool $Name
    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    Copy-Item -LiteralPath $source -Destination $destination -Force

    $versionOutput = @(& $destination -version 2>&1)
    $exitCode = $LASTEXITCODE
    $toolName = [IO.Path]::GetFileNameWithoutExtension($Name)
    if ($exitCode -ne 0 -or ($versionOutput -join "`n") -notmatch "(?m)^$toolName version ") {
        $details = ($versionOutput | Select-Object -First 3) -join " | "
        throw "Bundled media tool validation failed for $Name (exit $exitCode): $details"
    }
}

Invoke-Step "Prepare isolated packaged-app smoke test" {
    # A browser's persistent Native Messaging connection may immediately relaunch
    # a user's installed app. The smoke process uses its own port and explicitly
    # bypasses only its single-instance guard, so building never closes a real
    # download or tampers with live browser integration.
}

Invoke-Step "Prepare directories" {
    Remove-Item -Recurse -Force $StageDir, $PortableStage -ErrorAction SilentlyContinue
    # Allows a resumed release build to refresh the portable/extension assets
    # after NSIS already produced a verified installer. The installer is kept
    # outside the cleaned release directory and copied back below; this avoids
    # an expensive second compression pass and never reuses an arbitrary file.
    if ($SkipInstaller) {
        if (-not (Test-Path -LiteralPath $InstallerOut)) {
            throw "-SkipInstaller requires an existing installer: $InstallerOut"
        }
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $PreservedInstaller) | Out-Null
        Copy-Item -LiteralPath $InstallerOut -Destination $PreservedInstaller -Force
    }
    if (Test-Path -LiteralPath $ReleaseDir) {
        $resolvedRelease = (Resolve-Path -LiteralPath $ReleaseDir).Path
        if ((Split-Path $resolvedRelease -Parent) -ne $Root.Path -or (Split-Path $resolvedRelease -Leaf) -ne "release") {
            throw "Refusing to clean unexpected release directory: $resolvedRelease"
        }
        Remove-Item -LiteralPath $resolvedRelease -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $StageDir, $ReleaseDir, $BinDir, $ToolsDir | Out-Null
    if ($SkipInstaller) {
        Copy-Item -LiteralPath $PreservedInstaller -Destination $InstallerOut -Force
    }
    if (-not (Test-Path -LiteralPath $IconFile)) {
        throw "Application icon is missing: $IconFile"
    }
}

Invoke-Step "Prepare FFmpeg tools" {
    Copy-MediaTool "ffmpeg.exe"
    Copy-MediaTool "ffprobe.exe"
}

if (-not $SkipFrontend) {
    Invoke-Step "Build frontend" {
        Push-Location $FrontendDir
        try {
            if (-not (Test-Path "node_modules")) {
                pnpm install --frozen-lockfile
            }
            pnpm run build
        } finally {
            Pop-Location
        }
    }
    Invoke-Step "Build browser extensions" {
        Push-Location $ExtensionDir
        $previousExtensionVersion = $env:HLS_EXTENSION_VERSION
        try {
            if ($IncludeExtensionAssets) {
                $env:HLS_EXTENSION_VERSION = $Version
            }
            if (-not (Test-Path "node_modules")) { pnpm install --frozen-lockfile }
            pnpm test
            pnpm run build:chrome
            Remove-Item -Recurse -Force $ChromiumExtensionStage -ErrorAction SilentlyContinue
            New-Item -ItemType Directory -Force -Path $ChromiumExtensionStage | Out-Null
            $chromeManifest = Get-Content -LiteralPath .output/chrome-mv3/manifest.json -Raw -Encoding UTF8 | ConvertFrom-Json
            if ($chromeManifest.manifest_version -ne 3) {
                throw "Chrome/Edge extension build did not produce Manifest V3"
            }
            Copy-Item -Recurse -Force -Path .output/chrome-mv3/* -Destination $ChromiumExtensionStage
            Remove-Item -Recurse -Force $FirefoxStage -ErrorAction SilentlyContinue
            pnpm run build:firefox
            pnpm exec web-ext lint --source-dir .output/firefox-mv3 --warnings-as-errors
            $manifest = Get-Content -LiteralPath .output/firefox-mv3/manifest.json -Raw -Encoding UTF8 | ConvertFrom-Json
            if ($manifest.browser_specific_settings.gecko.id -ne $FirefoxId) {
                throw "Firefox build used the wrong extension ID"
            }
            $mediaScript = @($manifest.content_scripts | Where-Object { $_.js -contains "content-scripts/content.js" })
            $hookScript = @($manifest.content_scripts | Where-Object { $_.js -contains "content-scripts/hooks.js" })
            if ($mediaScript.Count -ne 1 -or $mediaScript[0].all_frames -ne $true -or $hookScript.Count -ne 1 -or $hookScript[0].all_frames -ne $true) {
                throw "Firefox build does not capture media in every frame"
            }
            New-Item -ItemType Directory -Force -Path $FirefoxStage | Out-Null
            Copy-Item -Recurse -Force -Path .output/firefox-mv3/* -Destination $FirefoxStage
        } finally {
            $env:HLS_EXTENSION_VERSION = $previousExtensionVersion
            Pop-Location
        }
    }
}

if (-not $SkipDesktop) {
    Invoke-Step "Build Tauri desktop shell" {
        Push-Location $FrontendDir
        $previousCargoLto = $env:CARGO_PROFILE_RELEASE_LTO
        $previousCargoCodegenUnits = $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS
        try {
            if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
                throw "Rust/Cargo is required to build the Tauri desktop shell. Install rustup before packaging."
            }
            # The default release profile is too aggressive for this Windows
            # toolchain.  LTO was causing the Tauri build to fail in this
            # environment even though the source itself compiled cleanly.
            $env:CARGO_PROFILE_RELEASE_LTO = "false"
            $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "16"
            pnpm run tauri:build
            if ($LASTEXITCODE -ne 0) { throw "Tauri desktop build failed with exit code $LASTEXITCODE" }
        } finally {
            $env:CARGO_PROFILE_RELEASE_LTO = $previousCargoLto
            $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = $previousCargoCodegenUnits
            Pop-Location
        }
    }
}

if (-not $SkipBackend) {
Invoke-Step "Build backend executable" {
    New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($PyInstallerVersionFile)) | Out-Null
    $numericFileVersion = ($versionParts | ForEach-Object { [int]$_ }) -join ", "
    $pyInstallerVersion = @"
VSVersionInfo(
  ffi=FixedFileInfo(
    filevers=($numericFileVersion),
    prodvers=($numericFileVersion),
    mask=0x3f,
    flags=0x0,
    OS=0x40004,
    fileType=0x1,
    subtype=0x0,
    date=(0, 0)
  ),
  kids=[
    StringFileInfo([
      StringTable('040904B0', [
        StringStruct('CompanyName', 'HLS Downloader'),
        StringStruct('FileDescription', 'HLS Downloader background component'),
        StringStruct('FileVersion', '$Version'),
        StringStruct('ProductName', 'HLS Downloader'),
        StringStruct('ProductVersion', '$Version')
      ])
    ]),
    VarFileInfo([VarStruct('Translation', [1033, 1200])])
  ]
)
"@
    Write-Utf8NoBom $PyInstallerVersionFile $pyInstallerVersion
    Push-Location $BackendDir
    $previousPythonPath = $env:PYTHONPATH
    try {
        # Keep unrelated local projects out of PyInstaller's module graph.
        $env:PYTHONPATH = ""
        Invoke-PyInstallerWithRetry "Core executable" {
            python -m PyInstaller `
                --noconfirm `
                --clean `
                --onedir `
                --noconsole `
                --name HLSDownloaderCore `
                --icon $IconFile `
                --version-file $PyInstallerVersionFile `
                --paths . `
                --collect-all curl_cffi `
                --collect-all libtorrent `
                --collect-all yt_dlp `
                --collect-all multipart `
                --collect-all pychromecast `
                --collect-all zeroconf `
                --collect-all casttube `
                --hidden-import uvicorn.lifespan.on `
                --hidden-import uvicorn.loops.auto `
                --hidden-import uvicorn.protocols.http.auto `
                --hidden-import uvicorn.protocols.websockets.auto `
                run_core.py
        }
        Invoke-PyInstallerWithRetry "Native host executable" {
            python -m PyInstaller `
                --noconfirm `
                --clean `
                --onefile `
                --console `
                --name HLSDownloaderNativeHost `
                --icon $IconFile `
                --version-file $PyInstallerVersionFile `
                native_host.py
        }
    } finally {
        $env:PYTHONPATH = $previousPythonPath
        Pop-Location
    }
}
}

Invoke-Step "Stage application files" {
    $tauriExecutable = Join-Path $FrontendDir "src-tauri\target\release\HLSDownloader.exe"
    if (-not (Test-Path -LiteralPath $tauriExecutable)) {
        throw "Tauri desktop executable is missing: $tauriExecutable"
    }
    Copy-Item -LiteralPath $tauriExecutable -Destination (Join-Path $StageDir "HLSDownloader.exe")
    Copy-Item -Path (Join-Path $BackendDir "dist\HLSDownloaderCore\*") -Destination $StageDir -Recurse -Force
    Copy-Item -Path (Join-Path $BackendDir "dist\HLSDownloaderNativeHost.exe") -Destination $StageDir
    Invoke-Step "Build native supervisor" {
        if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
            throw "Rust/Cargo is required to build HLSNativeShell.exe"
        }
        $nativeShellDir = Join-Path $Root "native_shell"
        Push-Location $nativeShellDir
        try {
            cargo build --release --locked
            if ($LASTEXITCODE -ne 0) { throw "native supervisor build failed with exit code $LASTEXITCODE" }
        } finally {
            Pop-Location
        }
        $builtShell = Join-Path $nativeShellDir "target\release\hls-native-shell.exe"
        if (-not (Test-Path -LiteralPath $builtShell)) {
            throw "native supervisor binary is missing: $builtShell"
        }
        Copy-Item -LiteralPath $builtShell -Destination (Join-Path $StageDir "HLSNativeShell.exe")
    }
    Copy-Item -LiteralPath (Join-Path $Root "config.default.json") -Destination (Join-Path $StageDir "config.json")
    Copy-Item -LiteralPath (Join-Path $Root "LICENSE") -Destination (Join-Path $StageDir "LICENSE.txt")
    Copy-Item -LiteralPath (Join-Path $Root "TERMS.md") -Destination (Join-Path $StageDir "TERMS.md")
    Copy-Item -LiteralPath (Join-Path $Root "PRIVACY.md") -Destination (Join-Path $StageDir "PRIVACY.md")
    Copy-Item -LiteralPath (Join-Path $Root "THIRD_PARTY_NOTICES.md") -Destination (Join-Path $StageDir "THIRD_PARTY_NOTICES.md")
    # MUI's license page consumes Windows Unicode text.  Keep TERMS.md UTF-8
    # for the application/API, and generate a UTF-16LE installer copy in both
    # Windows PowerShell 5.1 and PowerShell 7 without relying on shell defaults.
    [IO.File]::WriteAllText(
        (Join-Path $StageDir "TERMS.txt"),
        [IO.File]::ReadAllText((Join-Path $Root "TERMS.md"), [Text.Encoding]::UTF8),
        [Text.Encoding]::Unicode
    )
    python (Join-Path $Root "scripts\generate_sbom.py") --version $Version --output (Join-Path $StageDir "sbom.cdx.json")
    if ($LASTEXITCODE -ne 0) { throw "SBOM generation failed with exit code $LASTEXITCODE" }

    New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "assets") | Out-Null
    Copy-Item -Path (Join-Path $AssetsDir "app-icon.png") -Destination (Join-Path $StageDir "assets")
    Copy-Item -Path $IconFile -Destination (Join-Path $StageDir "assets")

    New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "bin") | Out-Null
    Copy-Item -Path (Join-Path $Root "bin\ffmpeg.exe") -Destination (Join-Path $StageDir "bin")
    Copy-Item -Path (Join-Path $Root "bin\ffprobe.exe") -Destination (Join-Path $StageDir "bin")

    New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "frontend") | Out-Null
    Copy-Item -Recurse -Force -Path (Join-Path $FrontendDir "dist") -Destination (Join-Path $StageDir "frontend")

    $bundledChromeExtension = Join-Path $StageDir "browser-extension\chrome"
    New-Item -ItemType Directory -Force -Path $bundledChromeExtension | Out-Null
    Copy-Item -Recurse -Force -Path (Join-Path $ExtensionDir ".output\chrome-mv3\*") -Destination $bundledChromeExtension

    New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "native-host") | Out-Null
    Copy-Item -Force -Path (Join-Path $ExtensionDir "native-host\chrome.json"), (Join-Path $ExtensionDir "native-host\firefox.json") -Destination (Join-Path $StageDir "native-host")
    New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "scripts") | Out-Null
    Copy-Item -Force -Path `
        (Join-Path $Root "scripts\register-native-host.ps1"), `
        (Join-Path $Root "scripts\shutdown-running.ps1"), `
        (Join-Path $Root "scripts\upgrade-portable.ps1") `
        -Destination (Join-Path $StageDir "scripts")
}

if (-not $SkipSmoke) {
    Invoke-Step "Smoke test packaged app" {
        $smokeExe = Join-Path $StageDir "HLSDownloader.exe"
        $smokePortableMarker = Join-Path $StageDir "portable"
        $smokeListener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
        $smokeListener.Start()
        $smokePort = ([System.Net.IPEndPoint]$smokeListener.LocalEndpoint).Port
        $smokeListener.Stop()
        $smokeApiBase = "http://127.0.0.1:$smokePort"
        Set-Content -LiteralPath $smokePortableMarker -Value "" -Encoding ASCII
        try {
            # The staged smoke copy is intentionally isolated from the user's
            # install. Seed it with a fresh credential so the health/API checks
            # exercise authenticated startup even when config.default.json has
            # no token (the release package must not contain a reusable token).
            $smokeConfigPath = Join-Path $StageDir "config.json"
            $smokeConfig = Get-Content -LiteralPath $smokeConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
            $smokeTokenBytes = New-Object byte[] 32
            $smokeRng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
            try { $smokeRng.GetBytes($smokeTokenBytes) } finally { $smokeRng.Dispose() }
            $smokeConfig | Add-Member -NotePropertyName token -NotePropertyValue ([Convert]::ToBase64String($smokeTokenBytes)) -Force
            $smokeConfig | Add-Member -NotePropertyName port -NotePropertyValue $smokePort -Force
            Write-Utf8NoBom $smokeConfigPath ($smokeConfig | ConvertTo-Json -Depth 20)
            $previousBuildSmoke = $env:HLS_DOWNLOADER_BUILD_SMOKE
            $env:HLS_DOWNLOADER_BUILD_SMOKE = "1"
            $proc = Start-Process -FilePath $smokeExe -WorkingDirectory $StageDir -PassThru -WindowStyle Hidden
            try {
                $ok = $false
                # Fresh PyInstaller executables can spend tens of seconds in
                # Defender/SmartScreen scanning on slower Windows machines.
                # Keep the wait bounded but do not classify that cold-start
                # delay as a broken package. A real desktop crash still exits
                # the loop immediately and reports process diagnostics below.
                for ($i = 0; $i -lt 120; $i++) {
                    Start-Sleep -Milliseconds 500
                    try {
                        $health = Invoke-RestMethod -Uri "$smokeApiBase/api/health" -TimeoutSec 1
                        if ($health) {
                            $ok = $true
                            break
                        }
                    } catch {
                    }
                    $proc.Refresh()
                    if ($proc.HasExited) { break }
                }
                if (-not $ok) {
                    $proc.Refresh()
                    $stageCoreExe = Join-Path $StageDir "HLSDownloaderCore.exe"
                    $stageCoreCount = @(Get-Process HLSDownloaderCore -ErrorAction SilentlyContinue |
                        Where-Object { $_.Path -eq $stageCoreExe }).Count
                    $coreLogPath = Join-Path $StageDir "core.log"
                    $coreErrorLogPath = Join-Path $StageDir "core-error.log"
                    $diagnostics = @(
                        "desktop_exited=$($proc.HasExited)",
                        "desktop_exit_code=$(if ($proc.HasExited) { $proc.ExitCode } else { 'running' })",
                        "core_processes=$stageCoreCount",
                        "core_log_bytes=$(if (Test-Path -LiteralPath $coreLogPath) { (Get-Item -LiteralPath $coreLogPath).Length } else { 0 })",
                        "core_error_log_bytes=$(if (Test-Path -LiteralPath $coreErrorLogPath) { (Get-Item -LiteralPath $coreErrorLogPath).Length } else { 0 })"
                    ) -join ", "
                    throw "Packaged app did not respond on /api/health ($diagnostics)"
                }
                $packagedConfigPath = Join-Path $StageDir "config.json"
                $packagedConfig = Get-Content -LiteralPath $packagedConfigPath -Raw -Encoding UTF8 | ConvertFrom-Json
                $packagedToken = [string]$packagedConfig.token
                if ($packagedToken.Length -lt 32 -or $packagedToken -eq "55555") {
                    throw "Packaged app did not generate a secure internal credential"
                }
                $stageCoreExe = Join-Path $StageDir "HLSDownloaderCore.exe"
                $stageCore = @(Get-Process HLSDownloaderCore -ErrorAction SilentlyContinue |
                    Where-Object { $_.Path -eq $stageCoreExe })
                if (-not $stageCore.Count) {
                    throw "Smoke test reached a different HLS Downloader core instead of the staged build."
                }
                $packagedSettings = Invoke-RestMethod `
                    -Uri "$smokeApiBase/api/settings" `
                    -Headers @{ "X-Token" = $packagedToken } `
                    -TimeoutSec 2
                if ($null -ne $packagedSettings.PSObject.Properties["token"]) {
                    throw "Internal credential leaked through the Settings API"
                }
                foreach ($field in @(
                    "http_chunk_size_mb",
                    "bt_upload_limit_kib",
                    "bt_max_connections",
                    "bt_enable_dht",
                    "browser_takeover_enabled",
                    "browser_takeover_min_mb"
                )) {
                    if ($null -eq $packagedSettings.PSObject.Properties[$field]) {
                        throw "Packaged Settings schema is missing field: $field"
                    }
                }

                $nativeRegistrationScript = Join-Path $StageDir "scripts\register-native-host.ps1"
                $nativeRegistryPrefix = "HKCU:\Software\HLSDownloaderBuildSmoke"
                & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $nativeRegistrationScript -RegistryPrefix $nativeRegistryPrefix
                if ($LASTEXITCODE -ne 0) {
                    throw "Native Messaging registration smoke test failed"
                }
                $expectedNativeHost = Join-Path $StageDir "HLSDownloaderNativeHost.exe"
                foreach ($manifestName in @("chrome.json", "firefox.json")) {
                    $manifestPath = Join-Path $StageDir "native-host\$manifestName"
                    $manifestJson = [System.IO.File]::ReadAllText($manifestPath, [System.Text.Encoding]::UTF8)
                    $manifest = $manifestJson | ConvertFrom-Json
                    if ($manifest.path -ne $expectedNativeHost) {
                        throw "Native Messaging manifest contains the wrong host path: $($manifest.path)"
                    }
                }
                python (Join-Path $Root "scripts\smoke_native_host.py") --exe $expectedNativeHost
                if ($LASTEXITCODE -ne 0) {
                    throw "Native Messaging protocol smoke test failed"
                }

                # HLS_DOWNLOADER_BUILD_SMOKE deliberately isolates this staged
                # process from an already-running user install, which means it
                # cannot also exercise the production single-instance mutex.
                # The normal production path retains tauri-plugin-single-instance
                # and is type-checked with the desktop build above.
                if (-not $env:HLS_DOWNLOADER_BUILD_SMOKE) {
                    $baselineProcesses = @(Get-Process HLSDownloader -ErrorAction SilentlyContinue |
                        Where-Object { $_.Path -eq $smokeExe })
                    if ($baselineProcesses.Count -lt 1 -or $baselineProcesses.Count -gt 2) {
                        throw "Single-instance check failed: unexpected primary process count $($baselineProcesses.Count)"
                    }
                    $secondProc = Start-Process -FilePath $smokeExe -WorkingDirectory $StageDir -PassThru -WindowStyle Hidden
                    if (-not $secondProc.WaitForExit(12000)) {
                        Stop-Process -Id $secondProc.Id -Force -ErrorAction SilentlyContinue
                        throw "Single-instance check failed: second packaged process did not exit"
                    }
                    $proc.Refresh()
                    $samePathProcesses = @(Get-Process HLSDownloader -ErrorAction SilentlyContinue |
                        Where-Object { $_.Path -eq $smokeExe })
                    if ($proc.HasExited -or $samePathProcesses.Count -ne $baselineProcesses.Count) {
                        throw "Single-instance check failed: second launch changed the packaged process count"
                    }
                }

                # Exercise the exact bounded helper embedded in the NSIS
                # installer, not a separate test-only shutdown path.
                $shutdownHelper = Join-Path $StageDir "scripts\shutdown-running.ps1"
                & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass `
                    -File $shutdownHelper -InstallDir $StageDir -TimeoutSeconds 12
                if ($LASTEXITCODE -ne 0) {
                    throw "Installer shutdown helper failed with exit code $LASTEXITCODE"
                }

                # A signed/unsigned executable may be scanned by Windows
                # Defender immediately after PyInstaller writes it. The helper
                # already bounded the wait, but independently verify that every
                # same-path process disappeared and its executable is writable.
                for ($i = 0; $i -lt 20; $i++) {
                    $proc.Refresh()
                    $remaining = Get-Process HLSDownloader -ErrorAction SilentlyContinue |
                        Where-Object { $_.Path -eq $smokeExe }
                    if ($proc.HasExited -and -not $remaining) {
                        break
                    }
                    Start-Sleep -Milliseconds 250
                }
                $proc.Refresh()
                if (-not $proc.HasExited -or (Get-Process HLSDownloader -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $smokeExe })) {
                    throw "Installer shutdown helper left a packaged app process running"
                }
            } finally {
                if ($proc -and -not $proc.HasExited) {
                    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
                }
                for ($i = 0; $i -lt 20; $i++) {
                    $children = Get-Process HLSDownloader -ErrorAction SilentlyContinue |
                        Where-Object { $_.Path -eq $smokeExe }
                    if (-not $children) {
                        break
                    }
                    $children | Stop-Process -Force -ErrorAction SilentlyContinue
                    Start-Sleep -Milliseconds 250
                }
                if (Get-Process HLSDownloader -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $smokeExe }) {
                    throw "Packaged app processes remained after smoke test"
                }
                for ($i = 0; $i -lt 80; $i++) {
                    if (-not (Get-NetTCPConnection -LocalPort $smokePort -State Listen -ErrorAction SilentlyContinue)) {
                        break
                    }
                    $listeners = @(Get-NetTCPConnection -LocalPort $smokePort -State Listen -ErrorAction SilentlyContinue)
                    foreach ($listener in $listeners) {
                        $owner = Get-Process -Id $listener.OwningProcess -ErrorAction SilentlyContinue
                        if ($owner -and $owner.ProcessName -in @("HLSDownloader", "HLSDownloaderCore")) {
                            $owner | Stop-Process -Force -ErrorAction SilentlyContinue
                        }
                    }
                    Start-Sleep -Milliseconds 250
                }
                if (Get-NetTCPConnection -LocalPort $smokePort -State Listen -ErrorAction SilentlyContinue) {
                    throw "Smoke port $smokePort remained occupied after smoke test"
                }
            }
            } finally {
                $env:HLS_DOWNLOADER_BUILD_SMOKE = $previousBuildSmoke
                $nativeRegistrationScript = Join-Path $StageDir "scripts\register-native-host.ps1"
            if (Test-Path -LiteralPath $nativeRegistrationScript) {
                & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $nativeRegistrationScript -Unregister -RegistryPrefix "HKCU:\Software\HLSDownloaderBuildSmoke" | Out-Null
            }
            # The unregister helper removes every host leaf. Remove the empty
            # isolated parent tree as well so repeated release builds leave no
            # smoke-only registry keys on the developer machine.
            Remove-Item -LiteralPath "HKCU:\Software\HLSDownloaderBuildSmoke" -Recurse -Force -ErrorAction SilentlyContinue
            Copy-Item -Force -Path `
                (Join-Path $ExtensionDir "native-host\chrome.json"), `
                (Join-Path $ExtensionDir "native-host\firefox.json") `
                -Destination (Join-Path $StageDir "native-host")
            Remove-Item -LiteralPath $smokePortableMarker -Force -ErrorAction SilentlyContinue
            Copy-Item -Force -LiteralPath (Join-Path $Root "config.default.json") -Destination (Join-Path $StageDir "config.json")
            Remove-Item -LiteralPath (Join-Path $StageDir "data.db"), (Join-Path $StageDir "data.db-shm"), (Join-Path $StageDir "data.db-wal") -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath (Join-Path $StageDir "core.log"), (Join-Path $StageDir "core-error.log") -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath (Join-Path $StageDir "downloads") -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

if (-not $SkipInstaller) {
    Invoke-Step "Build NSIS installer" {
        $makensis = Get-MakeNsis
        & $makensis "/INPUTCHARSET" "UTF8" "/DAPP_VERSION=$Version" "/DAPP_FILE_VERSION=$FileVersion" "/DSTAGE_DIR=$StageDir" "/DICON_FILE=$IconFile" "/DOUT_FILE=$InstallerOut" $InstallerScript
        if ($LASTEXITCODE -ne 0) {
            throw "makensis failed with exit code $LASTEXITCODE"
        }
        if (-not (Test-Path $InstallerOut)) {
            throw "makensis reported success but did not create $InstallerOut"
        }
    }
    if (-not $SkipSmoke) {
        Invoke-Step "Smoke test installer cover upgrade and uninstall" {
            & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass `
                -File (Join-Path $Root "scripts\smoke-installer-upgrade.ps1") `
                -InstallerPath $InstallerOut
            if ($LASTEXITCODE -ne 0) {
                throw "Installer upgrade/uninstall smoke test failed with exit code $LASTEXITCODE"
            }
        }
    }
}

Invoke-Step "Build portable archive" {
    New-Item -ItemType Directory -Force -Path $PortableStage | Out-Null
    Copy-Item -Path (Join-Path $StageDir "*") -Destination $PortableStage -Recurse -Force
    # Match the installer behaviour for a portable-over-portable upgrade.  An
    # older browser may keep the legacy root host locked, so the archive ships
    # a fresh versioned host instead of asking Explorer to overwrite it.
    $portableNativeHost = Join-Path $PortableStage "HLSDownloaderNativeHost.exe"
    $portableVersionsDir = Join-Path $PortableStage "native-host\versions"
    $portableVersionedHost = Join-Path $portableVersionsDir "HLSDownloaderNativeHost-$Version.exe"
    New-Item -ItemType Directory -Force -Path $portableVersionsDir | Out-Null
    Move-Item -LiteralPath $portableNativeHost -Destination $portableVersionedHost -Force
    $portableManifestsDir = Join-Path $PortableStage "native-host\manifests"
    New-Item -ItemType Directory -Force -Path $portableManifestsDir | Out-Null
    Move-Item -LiteralPath (Join-Path $PortableStage "native-host\chrome.json") -Destination (Join-Path $portableManifestsDir "chrome-$Version.json") -Force
    Move-Item -LiteralPath (Join-Path $PortableStage "native-host\firefox.json") -Destination (Join-Path $portableManifestsDir "firefox-$Version.json") -Force
    Set-Content -LiteralPath (Join-Path $PortableStage "portable") -Value "" -Encoding ASCII
    $portableReadme = @(
        'HLS Downloader portable edition',
        '',
        'Run HLSDownloader.exe. The application uses the Microsoft Edge WebView2',
        'runtime that is included with supported Windows 10/11 installations.',
        '',
        'To enable Chrome/Firefox integration, run:',
        'powershell -ExecutionPolicy Bypass -File scripts\register-native-host.ps1',
        'To remove the registration, add -Unregister.',
        '',
        'To upgrade an existing portable copy safely, extract this archive into a new',
        'folder, then run:',
        'powershell -ExecutionPolicy Bypass -File scripts\upgrade-portable.ps1 -TargetDir "C:\path\to\old\HLS Downloader" -StartAfterUpgrade',
        '',
        'For Chrome, open chrome://extensions, enable Developer mode, choose Load unpacked,',
        'then select browser-extension\chrome.'
    )
    $portableReadme | Set-Content -LiteralPath (Join-Path $PortableStage "README-PORTABLE.txt") -Encoding UTF8
    Compress-Archive -Path (Join-Path $PortableStage "*") -DestinationPath $PortableOut -CompressionLevel Optimal
    if (-not (Test-Path -LiteralPath $PortableOut)) {
        throw "Portable archive was not created: $PortableOut"
    }
}

Invoke-Step "Assemble release files" {
    $expected = @($InstallerOut, $PortableOut)
    if ($IncludeExtensionAssets) {
        Compress-Archive -Path (Join-Path $ChromiumExtensionStage "*") -DestinationPath $ChromiumExtensionOut -CompressionLevel Optimal
        Compress-Archive -Path (Join-Path $FirefoxStage "*") -DestinationPath $FirefoxExtensionOut -CompressionLevel Optimal
        $sourceInputs = @(
            (Join-Path $ExtensionDir "entrypoints"),
            (Join-Path $ExtensionDir "lib"),
            (Join-Path $ExtensionDir "native-host"),
            (Join-Path $ExtensionDir "public"),
            (Join-Path $ExtensionDir "AMO-BUILD.md"),
            (Join-Path $ExtensionDir "package.json"),
            (Join-Path $ExtensionDir "pnpm-lock.yaml"),
            (Join-Path $ExtensionDir "pnpm-workspace.yaml"),
            (Join-Path $ExtensionDir "tsconfig.json"),
            (Join-Path $ExtensionDir "wxt.config.ts"),
            (Join-Path $Root "PRIVACY.md"),
            (Join-Path $Root "TERMS.md"),
            (Join-Path $Root "THIRD_PARTY_NOTICES.md")
        )
        $sourceStage = Join-Path $ExtensionBuildDir "source"
        Remove-Item -Recurse -Force $sourceStage -ErrorAction SilentlyContinue
        New-Item -ItemType Directory -Force -Path $sourceStage | Out-Null
        Copy-Item -Recurse -Force -Path $sourceInputs -Destination $sourceStage
        $sourceInfo = @"
Firefox extension ID: $FirefoxId
Build command: pnpm run build:firefox

All Firefox release packages use this same ID. The desktop UI mode is selected by the app connection, not by a second extension identity.
"@
        Write-Utf8NoBom (Join-Path $sourceStage "BUILD-INFO.txt") $sourceInfo
        Compress-Archive -Path (Join-Path $sourceStage "*") -DestinationPath $FirefoxSourceOut -CompressionLevel Optimal
        $expected += @($ChromiumExtensionOut, $FirefoxExtensionOut, $FirefoxSourceOut)
    }
    foreach ($path in $expected) {
        if (-not (Test-Path -LiteralPath $path)) {
            throw "Missing release file: $path"
        }
    }
    $actual = @(Get-ChildItem -LiteralPath $ReleaseDir -File)
    if ($actual.Count -ne $expected.Count) { throw "Release directory must contain exactly $($expected.Count) files; found $($actual.Count)" }
}

Write-Host ""
Write-Host "Windows release assets created:" -ForegroundColor Green
Write-Host $InstallerOut
Write-Host $PortableOut
if ($IncludeExtensionAssets) {
    Write-Host $ChromiumExtensionOut
    Write-Host $FirefoxExtensionOut
    Write-Host $FirefoxSourceOut
}
