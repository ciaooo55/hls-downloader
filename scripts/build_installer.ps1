param(
    [switch]$SkipFrontend,
    [switch]$SkipSmoke,
    [string]$Version = "1.1.1"
)

$ErrorActionPreference = "Stop"

$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$FrontendDir = Join-Path $Root "frontend"
$BackendDir = Join-Path $Root "backend"
$UserscriptDir = Join-Path $Root "userscript"
$StageDir = Join-Path $Root "build\installer\stage"
$PortableStage = Join-Path $Root "build\installer\portable"
$ReleaseDir = Join-Path $Root "release"
$ToolsDir = Join-Path $Root "tools"
$BinDir = Join-Path $Root "bin"
$WebViewBootstrapper = Join-Path $ToolsDir "MicrosoftEdgeWebview2Setup.exe"
$WebViewBootstrapperUrl = "https://go.microsoft.com/fwlink/p/?LinkId=2124703"
$NsisCondaPrefix = Join-Path $ToolsDir "nsis-conda"
$NsisVersion = "3.12"
$NsisZip = Join-Path $ToolsDir "nsis-$NsisVersion.zip"
$NsisUrl = "https://downloads.sourceforge.net/project/nsis/NSIS%203/$NsisVersion/nsis-$NsisVersion.zip"
$InstallerScript = Join-Path $Root "installer\hls-downloader.nsi"
$InstallerOut = Join-Path $ReleaseDir "HLSDownloader-Windows-x64-Setup.exe"
$PortableOut = Join-Path $ReleaseDir "HLSDownloader-Windows-x64-Portable.zip"

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
    try {
        if (-not (Test-Path $NsisZip)) {
            Write-Host "Downloading NSIS $NsisVersion..."
            Invoke-WebRequest -Uri $NsisUrl -OutFile $NsisZip -MaximumRedirection 10
        }
        $signature = [System.IO.File]::ReadAllBytes($NsisZip)[0..3]
        $downloadedZip = ($signature[0] -eq 0x50 -and $signature[1] -eq 0x4B)
    } catch {
        $downloadedZip = $false
    }

    if ($downloadedZip) {
        Expand-Archive -Path $NsisZip -DestinationPath $ToolsDir -Force
        $makensis = Get-ChildItem -Path $ToolsDir -Recurse -Filter "makensis.exe" | Select-Object -First 1
        if ($makensis) {
            return $makensis.FullName
        }
    }

    Remove-Item -Force $NsisZip -ErrorAction SilentlyContinue
    Write-Host "SourceForge did not return a usable zip; creating project-local conda NSIS environment..."
    & conda create -y -p $NsisCondaPrefix "nsis=$NsisVersion" | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "conda failed to install NSIS into $NsisCondaPrefix"
    }
    $makensis = Get-ChildItem -Path $ToolsDir -Recurse -Filter "makensis.exe" | Select-Object -First 1
    if (-not $makensis) {
        throw "makensis.exe not found after installing NSIS into $ToolsDir"
    }
    return $makensis.FullName
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
    if (-not $command) {
        throw "$Name was not found. Install FFmpeg or place $Name in $BinDir before packaging."
    }
    return $command.Source
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
    Get-Process HLSDownloader -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
}

Invoke-Step "Prepare directories" {
    Remove-Item -Recurse -Force $StageDir, $PortableStage -ErrorAction SilentlyContinue
    Remove-Item -Force $InstallerOut, $PortableOut -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $StageDir, $ReleaseDir, $BinDir, $ToolsDir | Out-Null
}

Invoke-Step "Prepare FFmpeg tools" {
    Copy-MediaTool "ffmpeg.exe"
    Copy-MediaTool "ffprobe.exe"
}

Invoke-Step "Prepare WebView2 bootstrapper" {
    if (-not (Test-Path $WebViewBootstrapper)) {
        Invoke-WebRequest -Uri $WebViewBootstrapperUrl -OutFile $WebViewBootstrapper -MaximumRedirection 10
    }
    $signature = Get-AuthenticodeSignature $WebViewBootstrapper
    if ($signature.Status -ne "Valid" -or $signature.SignerCertificate.Subject -notmatch "Microsoft Corporation") {
        throw "WebView2 bootstrapper does not have a valid Microsoft signature"
    }
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
}

Invoke-Step "Build backend executable" {
    Push-Location $BackendDir
    try {
        python -m PyInstaller `
            --noconfirm `
            --clean `
            --onefile `
            --noconsole `
            --name HLSDownloader `
            --paths . `
            --collect-all webview `
            --collect-all pystray `
            --hidden-import pystray._win32 `
            --hidden-import webview.platforms.edgechromium `
            --hidden-import uvicorn.lifespan.on `
            --hidden-import uvicorn.loops.auto `
            --hidden-import uvicorn.protocols.http.auto `
            --hidden-import uvicorn.protocols.websockets.auto `
            run_server.py
    } finally {
        Pop-Location
    }
}

Invoke-Step "Stage application files" {
    Copy-Item -Path (Join-Path $BackendDir "dist\HLSDownloader.exe") -Destination $StageDir
    Copy-Item -Path (Join-Path $Root "config.json") -Destination $StageDir
    Copy-Item -Path $WebViewBootstrapper -Destination $StageDir

    New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "bin") | Out-Null
    Copy-Item -Path (Join-Path $Root "bin\ffmpeg.exe") -Destination (Join-Path $StageDir "bin")
    Copy-Item -Path (Join-Path $Root "bin\ffprobe.exe") -Destination (Join-Path $StageDir "bin")

    New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "frontend") | Out-Null
    Copy-Item -Recurse -Force -Path (Join-Path $FrontendDir "dist") -Destination (Join-Path $StageDir "frontend")

    New-Item -ItemType Directory -Force -Path (Join-Path $StageDir "userscript") | Out-Null
    Copy-Item -Force -Path (Join-Path $UserscriptDir "m3u8-sniffer.user.js") -Destination (Join-Path $StageDir "userscript")
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
                for ($i = 0; $i -lt 20; $i++) {
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
            Remove-Item -LiteralPath $smokePortableMarker -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath (Join-Path $StageDir "data.db"), (Join-Path $StageDir "data.db-shm"), (Join-Path $StageDir "data.db-wal") -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath (Join-Path $StageDir ".webview"), (Join-Path $StageDir "downloads") -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

Invoke-Step "Build NSIS installer" {
    $makensis = Get-MakeNsis
    & $makensis "/INPUTCHARSET" "UTF8" "/DAPP_VERSION=$Version" "/DSTAGE_DIR=$StageDir" "/DOUT_FILE=$InstallerOut" $InstallerScript
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

Run HLSDownloader.exe. If Microsoft Edge WebView2 Runtime is missing, run
MicrosoftEdgeWebview2Setup.exe once before starting the downloader.
"@ | Set-Content -LiteralPath (Join-Path $PortableStage "README-PORTABLE.txt") -Encoding UTF8
    Compress-Archive -Path (Join-Path $PortableStage "*") -DestinationPath $PortableOut -CompressionLevel Optimal
    if (-not (Test-Path -LiteralPath $PortableOut)) {
        throw "Portable archive was not created: $PortableOut"
    }
}

Write-Host ""
Write-Host "Windows release assets created:" -ForegroundColor Green
Write-Host $InstallerOut
Write-Host $PortableOut
