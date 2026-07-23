param(
    [switch]$SkipFrontend,
    [switch]$SkipBackend,
    [switch]$SkipSmoke,
    [string]$Version = "1.4.1"
)

$ErrorActionPreference = "Stop"

$versionParts = @($Version -split '\.')
$invalidVersionPart = @($versionParts | Where-Object { $_ -notmatch '^\d+$' }).Count -gt 0
if ($versionParts.Count -gt 4 -or $versionParts.Count -lt 1 -or $invalidVersionPart) {
    throw "Version must contain one to four numeric parts: $Version"
}
while ($versionParts.Count -lt 4) { $versionParts += "0" }
$FileVersion = ($versionParts | ForEach-Object { [int]$_ }) -join "."

$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$FrontendDir = Join-Path $Root "frontend"
$BackendDir = Join-Path $Root "backend"
$ExtensionDir = Join-Path $Root "extension"
$AssetsDir = Join-Path $Root "assets"
$IconFile = Join-Path $AssetsDir "app-icon.ico"
$StageDir = Join-Path $Root "build\installer\stage"
$PortableStage = Join-Path $Root "build\installer\portable"
$ReleaseDir = Join-Path $Root "release"
$ToolsDir = Join-Path $Root "tools"
$BinDir = Join-Path $Root "bin"
$FFmpegArchive = Join-Path $ToolsDir "ffmpeg-windows.zip"
$FFmpegToolsDir = Join-Path $ToolsDir "ffmpeg-windows"
$FFmpegArchiveUrl = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"
$NsisCondaPrefix = Join-Path $ToolsDir "nsis-conda"
$NsisVersion = "3.12"
$NsisZip = Join-Path $ToolsDir "nsis-$NsisVersion.zip"
$NsisUrl = "https://downloads.sourceforge.net/project/nsis/NSIS%203/$NsisVersion/nsis-$NsisVersion.zip"
$InstallerScript = Join-Path $Root "installer\hls-downloader.nsi"
$InstallerOut = Join-Path $ReleaseDir "HLSDownloader-Windows-x64-Setup.exe"
$PortableOut = Join-Path $ReleaseDir "HLSDownloader-Windows-x64-Portable.zip"
$ChromeExtensionOut = Join-Path $ReleaseDir "HLSDownloader-Chrome.zip"
$FirefoxExtensionOut = Join-Path $ReleaseDir "HLSDownloader-Firefox-Unsigned.zip"
$FirefoxSourceOut = Join-Path $ReleaseDir "HLSDownloader-Firefox-Source.zip"
$ChecksumsOut = Join-Path $ReleaseDir "SHA256SUMS.txt"

function Invoke-Step($Name, [scriptblock]$Block) {
    Write-Host ""
    Write-Host "==> $Name" -ForegroundColor Cyan
    & $Block
}

function Get-MakeNsis {
    $pathCommand = Get-Command "makensis.exe" -ErrorAction SilentlyContinue
    if ($pathCommand) {
        return $pathCommand.Source
    }

    $installedCandidate = @(
        (Join-Path ${env:ProgramFiles(x86)} "NSIS\makensis.exe"),
        (Join-Path $env:ProgramFiles "NSIS\makensis.exe")
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
    if ($installedCandidate) {
        return $installedCandidate
    }

    $existing = Get-ChildItem -Path $ToolsDir -Recurse -Filter "makensis.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($existing) {
        return $existing.FullName
    }

    New-Item -ItemType Directory -Force -Path $ToolsDir | Out-Null
    $downloadedZip = $false
    for ($attempt = 1; $attempt -le 3 -and -not $downloadedZip; $attempt++) {
        try {
            if (-not (Test-Path $NsisZip)) {
                Write-Host "Downloading NSIS $NsisVersion (attempt $attempt/3)..."
                Invoke-WebRequest -Uri $NsisUrl -OutFile $NsisZip -MaximumRedirection 10
            }
            $signature = [System.IO.File]::ReadAllBytes($NsisZip)[0..3]
            $downloadedZip = ($signature[0] -eq 0x50 -and $signature[1] -eq 0x4B)
        } catch {
            $downloadedZip = $false
        }
        if (-not $downloadedZip) {
            Remove-Item -Force $NsisZip -ErrorAction SilentlyContinue
            if ($attempt -lt 3) { Start-Sleep -Seconds (2 * $attempt) }
        }
    }

    if ($downloadedZip) {
        Expand-Archive -Path $NsisZip -DestinationPath $ToolsDir -Force
        $makensis = Get-ChildItem -Path $ToolsDir -Recurse -Filter "makensis.exe" | Select-Object -First 1
        if ($makensis) {
            return $makensis.FullName
        }
    }

    $choco = Get-Command "choco.exe" -ErrorAction SilentlyContinue
    if ($choco) {
        Write-Host "SourceForge did not return a usable zip; installing NSIS with Chocolatey..."
        & $choco.Source install nsis --yes --no-progress --limit-output | Out-Host
        if ($LASTEXITCODE -eq 0) {
            $chocoCandidate = @(
                (Join-Path ${env:ProgramFiles(x86)} "NSIS\makensis.exe"),
                (Join-Path $env:ProgramFiles "NSIS\makensis.exe")
            ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
            if ($chocoCandidate) { return $chocoCandidate }
        }
    }

    $conda = Get-Command "conda.exe" -ErrorAction SilentlyContinue
    if ($conda) {
        Write-Host "Installing NSIS into a project-local conda environment..."
        & $conda.Source create -y -p $NsisCondaPrefix "nsis=$NsisVersion" | Out-Host
        if ($LASTEXITCODE -eq 0) {
            $makensis = Get-ChildItem -Path $NsisCondaPrefix -Recurse -Filter "makensis.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
            if ($makensis) { return $makensis.FullName }
        }
    }
    throw "Unable to install NSIS: SourceForge, Chocolatey and conda methods all failed."
}

function Find-MediaTool($Name) {
    if ($env:ChocolateyInstall) {
        $chocolateyTools = Join-Path $env:ChocolateyInstall "lib\ffmpeg\tools"
        if (Test-Path -LiteralPath $chocolateyTools) {
            $packagedTool = Get-ChildItem -LiteralPath $chocolateyTools -Recurse -File -Filter $Name -ErrorAction SilentlyContinue |
                Sort-Object Length -Descending |
                Select-Object -First 1
            if ($packagedTool) {
                return $packagedTool.FullName
            }
        }
    }

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    New-Item -ItemType Directory -Force -Path $ToolsDir | Out-Null
    if (-not (Test-Path -LiteralPath $FFmpegToolsDir)) {
        Write-Host "Downloading verified Windows FFmpeg build..."
        Invoke-WebRequest -Uri $FFmpegArchiveUrl -OutFile $FFmpegArchive -MaximumRedirection 10
        $signature = [System.IO.File]::ReadAllBytes($FFmpegArchive)[0..3]
        if ($signature[0] -ne 0x50 -or $signature[1] -ne 0x4B) {
            throw "FFmpeg download did not return a zip archive."
        }
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
    if (-not (Test-Path -LiteralPath $destination)) {
        $source = Find-MediaTool $Name
        New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
        Copy-Item -LiteralPath $source -Destination $destination
    }

    $versionOutput = @(& $destination -version 2>&1)
    $exitCode = $LASTEXITCODE
    $toolName = [IO.Path]::GetFileNameWithoutExtension($Name)
    if ($exitCode -ne 0 -or ($versionOutput -join "`n") -notmatch "(?m)^$toolName version ") {
        $details = ($versionOutput | Select-Object -First 3) -join " | "
        throw "Bundled media tool validation failed for $Name (exit $exitCode): $details"
    }
}

Invoke-Step "Stop running packaged app" {
    Get-Process HLSDownloader,HLSDownloaderCore -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
}

Invoke-Step "Prepare directories" {
    Remove-Item -Recurse -Force $StageDir, $PortableStage -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $ReleaseDir) {
        $resolvedRelease = (Resolve-Path -LiteralPath $ReleaseDir).Path
        if ((Split-Path $resolvedRelease -Parent) -ne $Root.Path -or (Split-Path $resolvedRelease -Leaf) -ne "release") {
            throw "Refusing to clean unexpected release directory: $resolvedRelease"
        }
        Remove-Item -LiteralPath $resolvedRelease -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $StageDir, $ReleaseDir, $BinDir, $ToolsDir | Out-Null
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
        try {
            if (-not (Test-Path "node_modules")) { pnpm install --frozen-lockfile }
            pnpm test
            pnpm run build
        } finally { Pop-Location }
    }
}

Invoke-Step "Build Tauri desktop shell" {
    Push-Location $FrontendDir
    try {
        if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
            throw "Rust/Cargo is required to build the Tauri desktop shell. Install rustup before packaging."
        }
        pnpm run tauri:build
        if ($LASTEXITCODE -ne 0) { throw "Tauri desktop build failed with exit code $LASTEXITCODE" }
    } finally {
        Pop-Location
    }
}

if (-not $SkipBackend) {
Invoke-Step "Build backend executable" {
    Push-Location $BackendDir
    $previousPythonPath = $env:PYTHONPATH
    try {
        # Keep unrelated local projects out of PyInstaller's module graph.
        $env:PYTHONPATH = ""
        python -m PyInstaller `
            --noconfirm `
            --clean `
            --onedir `
            --noconsole `
            --name HLSDownloaderCore `
            --icon $IconFile `
            --paths . `
            --collect-all curl_cffi `
            --collect-all libtorrent `
            --collect-all yt_dlp `
            --collect-all multipart `
            --hidden-import uvicorn.lifespan.on `
            --hidden-import uvicorn.loops.auto `
            --hidden-import uvicorn.protocols.http.auto `
            --hidden-import uvicorn.protocols.websockets.auto `
            run_core.py
        python -m PyInstaller `
            --noconfirm `
            --clean `
            --onefile `
            --console `
            --name HLSDownloaderNativeHost `
            native_host.py
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
    Copy-Item -Path (Join-Path $Root "config.json") -Destination $StageDir

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
        (Join-Path $Root "scripts\shutdown-running.ps1") `
        -Destination (Join-Path $StageDir "scripts")
}

if (-not $SkipSmoke) {
    Invoke-Step "Smoke test packaged app" {
        $smokeExe = Join-Path $StageDir "HLSDownloader.exe"
        $smokePortableMarker = Join-Path $StageDir "portable"
        Set-Content -LiteralPath $smokePortableMarker -Value "" -Encoding ASCII
        try {
            $proc = Start-Process -FilePath $smokeExe -WorkingDirectory $StageDir -PassThru -WindowStyle Hidden
            try {
                $ok = $false
                for ($i = 0; $i -lt 40; $i++) {
                    Start-Sleep -Milliseconds 500
                    try {
                        $health = Invoke-RestMethod -Uri "http://127.0.0.1:8765/api/health" -TimeoutSec 2
                        if ($health) {
                            $ok = $true
                            break
                        }
                    } catch {
                    }
                }
                if (-not $ok) {
                    throw "Packaged app did not respond on /api/health"
                }
                $packagedSettings = Invoke-RestMethod `
                    -Uri "http://127.0.0.1:8765/api/settings" `
                    -Headers @{ "X-Token" = "55555" } `
                    -TimeoutSec 2
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

                $shutdownAccepted = $false
                for ($i = 0; $i -lt 240; $i++) {
                    try {
                        $shutdown = Invoke-RestMethod `
                            -Method Post `
                            -Uri "http://127.0.0.1:8765/api/app/shutdown" `
                            -Headers @{ "X-Token" = "55555" } `
                            -ContentType "application/json" `
                            -Body "{}" `
                            -TimeoutSec 2
                        if ($shutdown.ok -eq $true) {
                            $shutdownAccepted = $true
                            break
                        }
                    } catch {
                    }
                    Start-Sleep -Milliseconds 250
                }
                if (-not $shutdownAccepted) {
                    throw "Graceful shutdown failed: desktop callback was not ready after 60 seconds"
                }

                # A signed/unsigned executable may be scanned by Windows
                # Defender immediately after PyInstaller writes it. Allow the
                # desktop and core enough time to finish graceful shutdown,
                # while still requiring every same-path process to disappear.
                for ($i = 0; $i -lt 120; $i++) {
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
                    throw "Graceful shutdown failed: packaged app process remained after 30 seconds"
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
                    if (-not (Get-NetTCPConnection -LocalPort 8765 -State Listen -ErrorAction SilentlyContinue)) {
                        break
                    }
                    Start-Sleep -Milliseconds 250
                }
                if (Get-NetTCPConnection -LocalPort 8765 -State Listen -ErrorAction SilentlyContinue) {
                    throw "Port 8765 remained occupied after smoke test"
                }
            }
        } finally {
            $nativeRegistrationScript = Join-Path $StageDir "scripts\register-native-host.ps1"
            if (Test-Path -LiteralPath $nativeRegistrationScript) {
                & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $nativeRegistrationScript -Unregister -RegistryPrefix "HKCU:\Software\HLSDownloaderBuildSmoke" | Out-Null
            }
            Copy-Item -Force -Path `
                (Join-Path $ExtensionDir "native-host\chrome.json"), `
                (Join-Path $ExtensionDir "native-host\firefox.json") `
                -Destination (Join-Path $StageDir "native-host")
            Remove-Item -LiteralPath $smokePortableMarker -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath (Join-Path $StageDir "data.db"), (Join-Path $StageDir "data.db-shm"), (Join-Path $StageDir "data.db-wal") -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath (Join-Path $StageDir "downloads") -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

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

Invoke-Step "Build portable archive" {
    New-Item -ItemType Directory -Force -Path $PortableStage | Out-Null
    Copy-Item -Path (Join-Path $StageDir "*") -Destination $PortableStage -Recurse -Force
    Set-Content -LiteralPath (Join-Path $PortableStage "portable") -Value "" -Encoding ASCII
    @"
HLS Downloader portable edition

Run HLSDownloader.exe. The application uses the Microsoft Edge WebView2
runtime that is included with supported Windows 10/11 installations.

To enable Chrome/Firefox integration, run:
powershell -ExecutionPolicy Bypass -File scripts\register-native-host.ps1
To remove the registration, add -Unregister.

For Chrome, open chrome://extensions, enable Developer mode, choose Load unpacked,
then select browser-extension\chrome.
"@ | Set-Content -LiteralPath (Join-Path $PortableStage "README-PORTABLE.txt") -Encoding UTF8
    Compress-Archive -Path (Join-Path $PortableStage "*") -DestinationPath $PortableOut -CompressionLevel Optimal
    if (-not (Test-Path -LiteralPath $PortableOut)) {
        throw "Portable archive was not created: $PortableOut"
    }
}

Invoke-Step "Assemble release files" {
    Compress-Archive -Path (Join-Path $ExtensionDir ".output\chrome-mv3\*") -DestinationPath $ChromeExtensionOut -CompressionLevel Optimal
    Compress-Archive -Path (Join-Path $ExtensionDir ".output\firefox-mv3\*") -DestinationPath $FirefoxExtensionOut -CompressionLevel Optimal
    Compress-Archive -Path @(
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
        (Join-Path $Root "PRIVACY.md")
    ) -DestinationPath $FirefoxSourceOut -CompressionLevel Optimal
    $expected = @($InstallerOut, $PortableOut, $ChromeExtensionOut, $FirefoxExtensionOut, $FirefoxSourceOut)
    foreach ($path in $expected) {
        if (-not (Test-Path -LiteralPath $path)) {
            throw "Missing release file: $path"
        }
    }
    $lines = Get-ChildItem -LiteralPath $ReleaseDir -File |
        Where-Object Name -ne "SHA256SUMS.txt" |
        Sort-Object Name |
        ForEach-Object {
            $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            "$hash  $($_.Name)"
        }
    $lines | Set-Content -LiteralPath $ChecksumsOut -Encoding ASCII
}

Write-Host ""
Write-Host "Windows release assets created:" -ForegroundColor Green
Write-Host $InstallerOut
Write-Host $PortableOut
Write-Host $ChromeExtensionOut
Write-Host $FirefoxExtensionOut
Write-Host $FirefoxSourceOut
Write-Host $ChecksumsOut
