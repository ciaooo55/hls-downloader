param(
    [Parameter(Mandatory = $true)]
    [string]$ArchivePath
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath $ArchivePath)) {
    throw "Missing v6 portable archive: $ArchivePath"
}

$root = Join-Path ([IO.Path]::GetTempPath()) ("hls-v6-smoke-" + [guid]::NewGuid().ToString("n"))
New-Item -ItemType Directory -Force -Path $root | Out-Null
try {
    Expand-Archive -LiteralPath $ArchivePath -DestinationPath $root -Force
    $exe = Join-Path $root "HLSDownloader.exe"
    $hostExe = Join-Path $root "HLSDownloaderNativeHost.exe"
    foreach ($path in @($exe, $hostExe, (Join-Path $root "native-host\chrome.json"), (Join-Path $root "scripts\register-native-host.ps1"))) {
        if (-not (Test-Path -LiteralPath $path)) {
            throw "v6 package missing $path"
        }
    }
    $env:HLS_V6_SKIP_LEGAL = "1"
    $env:HLS_V6_PLAYER_NULL = "1"
    $output = & $exe --self-test
    if ($LASTEXITCODE -ne 0) {
        throw "HLSDownloader.exe --self-test failed with $LASTEXITCODE"
    }
    if ($output -notmatch "hls-native-ui/") {
        throw "unexpected --self-test output: $output"
    }
    Write-Host "v6 package smoke ok: $output"
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}
