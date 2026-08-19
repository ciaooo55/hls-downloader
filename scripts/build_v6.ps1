param(
    [string]$Version = "6.0.1",
    [string]$OutDir = "",
    [switch]$SkipBuild,
    [switch]$SkipZip,
    [switch]$SkipInstaller,
    [switch]$UseSystemFfmpeg
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$ReleaseDir = if ($OutDir) {
    if ([System.IO.Path]::IsPathRooted($OutDir)) { $OutDir } else { Join-Path $Root $OutDir }
} else {
    Join-Path $Root "release"
}
$StageDir = Join-Path $Root "build\v6\stage"
$PortableDir = Join-Path $Root "build\v6\portable"
$UiManifest = Join-Path $Root "native_ui\Cargo.toml"
$IconFile = Join-Path $Root "assets\app-icon.ico"
$TermsSource = Join-Path $Root "TERMS.md"
$ToolsDir = Join-Path $Root "tools"
$NsisVersion = "3.12"
$NsisZip = Join-Path $ToolsDir "nsis-$NsisVersion.zip"
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
$FFmpegArchive = Join-Path $ToolsDir "ffmpeg-windows.zip"
$FFmpegToolsDir = Join-Path $ToolsDir "ffmpeg-windows"
$FFmpegArchiveUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-19-19-21/ffmpeg-N-126217-ge1e325235e-win64-gpl.zip"
$FFmpegArchiveBuild = "BtbN autobuild 2026-08-19 19:21 (FFmpeg ge1e325235e)"
$FFmpegArchiveSha256 = "fe5a8f090b9fbc77d5e64c7d8b404b8837e05a09663ed9768ba19284cf929b20"
$LibMpvToolsDir = Join-Path $NsisRuntimeRoot "libmpv-20260814"
$SevenZipUrl = "https://github.com/ip7z/7zip/releases/download/26.02/7zr.exe"
$SevenZipSha256 = "56b8cc9f4971cef253644fafe54063ed7fdca551d4dee0f8c6baa81b855acd72"
$SevenZipExe = Join-Path $LibMpvToolsDir "7zr.exe"
$LibMpvArchiveUrl = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20260814/mpv-dev-x86_64-20260814-git-7b8915bc1d.7z"
$LibMpvArchiveBuild = "shinchiro mpv-dev x86_64 20260814 (git 7b8915bc1d)"
$LibMpvArchiveSha256 = "0af22b28e920620036d3ae08fd9283156dc9af0420bf4df84b0e02282094599c"
$LibMpvArchive = Join-Path $LibMpvToolsDir "mpv-dev-x86_64.7z"

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
            $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
            if ($curl) {
                & $curl.Source --location --fail --retry 3 --retry-delay 2 --max-time 600 --output $Path $Url
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
    $destination = Join-Path $StageDir $Name
    $source = Find-MediaTool $Name
    Copy-Item -LiteralPath $source -Destination $destination -Force
    $versionOutput = @(& $destination -version 2>&1)
    $exitCode = $LASTEXITCODE
    $toolName = [IO.Path]::GetFileNameWithoutExtension($Name)
    if ($exitCode -ne 0 -or ($versionOutput -join "`n") -notmatch "(?m)^$toolName version ") {
        $details = ($versionOutput | Select-Object -First 3) -join " | "
        throw "Bundled media tool validation failed for $Name (exit $exitCode): $details"
    }
}

function Copy-LibMpv {
    Get-VerifiedArchive $SevenZipUrl $SevenZipExe $SevenZipSha256 "7zr.exe"
    Get-VerifiedArchive $LibMpvArchiveUrl $LibMpvArchive $LibMpvArchiveSha256 "libmpv archive ($LibMpvArchiveBuild)"
    $extract = Join-Path $LibMpvToolsDir "extract"
    if (Test-Path -LiteralPath $extract) {
        Remove-Item -LiteralPath $extract -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $extract | Out-Null
    try {
        & $SevenZipExe e -y "-o$extract" $LibMpvArchive "libmpv-2.dll"
        if ($LASTEXITCODE -ne 0) {
            throw "7zr failed to extract libmpv-2.dll (exit $LASTEXITCODE)"
        }
        $dll = Get-ChildItem -LiteralPath $extract -Filter "libmpv-2.dll" -Recurse -File -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if (-not $dll) {
            throw "Pinned libmpv archive did not contain libmpv-2.dll"
        }
        Copy-Item -LiteralPath $dll.FullName -Destination (Join-Path $StageDir "libmpv-2.dll") -Force
    } finally {
        Remove-Item -LiteralPath $extract -Recurse -Force -ErrorAction SilentlyContinue
    }
}

New-Item -ItemType Directory -Force -Path $StageDir, $PortableDir, $ReleaseDir | Out-Null
Get-ChildItem -LiteralPath $StageDir -Force -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force

if (-not $SkipBuild) {
    Push-Location $Root
    try {
        cargo test --manifest-path (Join-Path $Root "native_shell\Cargo.toml") --locked --lib --no-default-features
        if ($LASTEXITCODE -ne 0) { throw "native_shell v6 Core tests failed" }
        cargo test --manifest-path (Join-Path $Root "native_shell\Cargo.toml") --locked
        if ($LASTEXITCODE -ne 0) { throw "native_shell tests failed" }
        cargo test --manifest-path $UiManifest --locked
        if ($LASTEXITCODE -ne 0) { throw "native_ui tests failed" }
        cargo build --manifest-path $UiManifest --locked --release --bin HLSDownloader
        if ($LASTEXITCODE -ne 0) { throw "native_ui release build failed" }
    } finally {
        Pop-Location
    }
}

$exe = Join-Path $Root "native_ui\target\release\HLSDownloader.exe"
if (-not (Test-Path -LiteralPath $exe)) {
    throw "Missing v6 binary: $exe"
}

Copy-Item -LiteralPath $exe -Destination (Join-Path $StageDir "HLSDownloader.exe") -Force
Copy-Item -LiteralPath $exe -Destination (Join-Path $StageDir "HLSDownloaderNativeHost.exe") -Force
Copy-MediaTool "ffmpeg.exe"
Copy-MediaTool "ffprobe.exe"
Copy-LibMpv
$curlNames = @("curl-impersonate.exe", "curl_chrome131.exe", "curl-impersonate-chrome.exe")
$curlCandidates = @()
if ($env:HLS_V6_CURL_IMPERSONATE) { $curlCandidates += $env:HLS_V6_CURL_IMPERSONATE }
foreach ($name in $curlNames) {
    $curlCandidates += @(
        (Join-Path $Root $name),
        (Join-Path (Split-Path $exe) $name)
    )
}
foreach ($curl in $curlCandidates) {
    if ($curl -and (Test-Path -LiteralPath $curl)) {
        Copy-Item -LiteralPath $curl -Destination (Join-Path $StageDir (Split-Path $curl -Leaf)) -Force
        break
    }
}
New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "native-host") | Out-Null
Copy-Item -LiteralPath (Join-Path $Root "extension\native-host\chrome.json") -Destination (Join-Path $StageDir "native-host\chrome.json") -Force
Copy-Item -LiteralPath (Join-Path $Root "extension\native-host\firefox.json") -Destination (Join-Path $StageDir "native-host\firefox.json") -Force
Copy-Item -LiteralPath (Join-Path $Root "extension\native-host\v6-chrome.json") -Destination (Join-Path $StageDir "native-host\v6-chrome.json") -Force
Copy-Item -LiteralPath (Join-Path $Root "extension\native-host\v6-firefox.json") -Destination (Join-Path $StageDir "native-host\v6-firefox.json") -Force
New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "scripts") | Out-Null
Copy-Item -LiteralPath (Join-Path $Root "scripts\register-native-host.ps1") -Destination (Join-Path $StageDir "scripts\register-native-host.ps1") -Force
Copy-Item -LiteralPath (Join-Path $Root "scripts\run_v6_gates.ps1") -Destination (Join-Path $StageDir "scripts\run_v6_gates.ps1") -Force
if (Test-Path -LiteralPath $TermsSource) {
    Copy-Item -LiteralPath $TermsSource -Destination (Join-Path $StageDir "TERMS.txt") -Force
}

$readme = @"
HLS Downloader v$Version (v6)

Launch HLSDownloader.exe. The same file copied as HLSDownloaderNativeHost.exe
is the Native Messaging host and does not open SQLite.

GitHub Windows Release ships this package as the product. Native Messaging
cutover is registered by Setup.exe. For a portable copy:

  powershell -ExecutionPolicy Bypass -File scripts\register-native-host.ps1 -Cutover
"@
[System.IO.File]::WriteAllText((Join-Path $StageDir "README.txt"), $readme, (New-Object System.Text.UTF8Encoding($false)))

if (-not $SkipZip) {
    if (Test-Path -LiteralPath $PortableDir) {
        Get-ChildItem -LiteralPath $PortableDir -Force -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $PortableDir | Out-Null
    Copy-Item -Path (Join-Path $StageDir "*") -Destination $PortableDir -Recurse -Force
    $zip = Join-Path $ReleaseDir "HLSDownloader-v$Version-Windows-x64-Portable.zip"
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    Compress-Archive -Path (Join-Path $PortableDir "*") -DestinationPath $zip
    Write-Host "Wrote $zip"
}

if (-not $SkipInstaller) {
    $nsi = Join-Path $Root "installer\hls-downloader-v6.nsi"
    if (-not (Test-Path -LiteralPath $IconFile)) {
        Write-Host "assets/app-icon.ico missing; skipped v6 Setup.exe"
    } elseif (-not (Test-Path -LiteralPath $nsi)) {
        Write-Host "installer/hls-downloader-v6.nsi missing; skipped v6 Setup.exe"
    } else {
        $makensis = Get-MakeNsis
        $out = Join-Path $ReleaseDir "HLSDownloader-v$Version-Windows-x64-Setup.exe"
        $stageNsis = ($StageDir -replace '\\', '/')
        $iconNsis = ($IconFile -replace '\\', '/')
        $outNsis = ($out -replace '\\', '/')
        $terms = Join-Path $StageDir "TERMS.txt"
        if (-not (Test-Path -LiteralPath $terms)) {
            [System.IO.File]::WriteAllText($terms, "HLS Downloader v6", (New-Object System.Text.UTF8Encoding($false)))
        }
        & $makensis `
            "/INPUTCHARSET" "UTF8" `
            "/DSTAGE_DIR=$stageNsis" `
            "/DOUT_FILE=$outNsis" `
            "/DICON_FILE=$iconNsis" `
            "/DAPP_VERSION=$Version" `
            $nsi
        if ($LASTEXITCODE -ne 0) { throw "makensis failed" }
        Write-Host "Wrote $out"
    }
}
