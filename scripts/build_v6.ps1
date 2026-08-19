param(
    [string]$Version = "6.0.0-dev",
    [string]$OutDir = "",
    [switch]$SkipBuild,
    [switch]$SkipZip,
    [switch]$SkipInstaller
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
$libmpvCandidates = @()
if ($env:HLS_V6_LIBMPV) { $libmpvCandidates += $env:HLS_V6_LIBMPV }
$libmpvCandidates += @(
    (Join-Path $Root "libmpv-2.dll"),
    (Join-Path $Root "native_ui\libmpv-2.dll"),
    (Join-Path (Split-Path $exe) "libmpv-2.dll")
)
foreach ($dll in $libmpvCandidates) {
    if ($dll -and (Test-Path -LiteralPath $dll)) {
        Copy-Item -LiteralPath $dll -Destination (Join-Path $StageDir "libmpv-2.dll") -Force
        break
    }
}
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
HLS Downloader v$Version (v6 portable)

Launch HLSDownloader.exe. The same file copied as HLSDownloaderNativeHost.exe
is the Native Messaging host and does not open SQLite.

Register the parallel v6 host name:
  powershell -ExecutionPolicy Bypass -File scripts\register-native-host.ps1 -V6

Cut the live 5.x host name over to this binary (after release gates):
  powershell -ExecutionPolicy Bypass -File scripts\register-native-host.ps1 -Cutover
"@
[System.IO.File]::WriteAllText((Join-Path $StageDir "README.txt"), $readme, (New-Object System.Text.UTF8Encoding($false)))

if (-not $SkipZip) {
    if (Test-Path -LiteralPath $PortableDir) {
        Get-ChildItem -LiteralPath $PortableDir -Force -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $PortableDir | Out-Null
    Copy-Item -Path (Join-Path $StageDir "*") -Destination $PortableDir -Recurse -Force
    $zip = Join-Path $ReleaseDir "HLSDownloader-v$Version-v6-Windows-x64-Portable.zip"
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    Compress-Archive -Path (Join-Path $PortableDir "*") -DestinationPath $zip
    Write-Host "Wrote $zip"
}

if (-not $SkipInstaller) {
    $makensis = Get-Command makensis -ErrorAction SilentlyContinue
    $nsi = Join-Path $Root "installer\hls-downloader-v6.nsi"
    if (-not $makensis) {
        Write-Host "makensis not on PATH; skipped v6 Setup.exe"
    } elseif (-not (Test-Path -LiteralPath $IconFile)) {
        Write-Host "assets/app-icon.ico missing; skipped v6 Setup.exe"
    } elseif (-not (Test-Path -LiteralPath $nsi)) {
        Write-Host "installer/hls-downloader-v6.nsi missing; skipped v6 Setup.exe"
    } else {
        $out = Join-Path $ReleaseDir "HLSDownloader-v$Version-v6-Windows-x64-Setup.exe"
        $stageNsis = ($StageDir -replace '\\', '/')
        $terms = Join-Path $StageDir "TERMS.txt"
        if (-not (Test-Path -LiteralPath $terms)) {
            [System.IO.File]::WriteAllText($terms, "HLS Downloader v6", (New-Object System.Text.UTF8Encoding($false)))
        }
        & $makensis.Source `
            "/DSTAGE_DIR=$stageNsis" `
            "/DOUT_FILE=$out" `
            "/DICON_FILE=$IconFile" `
            "/DAPP_VERSION=$Version" `
            $nsi
        if ($LASTEXITCODE -ne 0) { throw "makensis failed" }
        Write-Host "Wrote $out"
    }
}
