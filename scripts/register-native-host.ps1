param(
    [switch]$Unregister,
    [string]$RegistryPrefix = "HKCU:\Software"
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$name = "com.ciaooo55.hls_downloader"
$chrome = Join-Path $RegistryPrefix "Google\Chrome\NativeMessagingHosts\$name"
$firefox = Join-Path $RegistryPrefix "Mozilla\NativeMessagingHosts\$name"
if ($Unregister) {
    Remove-Item -LiteralPath $chrome, $firefox -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "Browser integration removed."
    exit 0
}
$manifestDir = Join-Path $root "native-host"
$hostExecutable = Join-Path $root "HLSDownloaderNativeHost.exe"
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
if (-not (Test-Path -LiteralPath $hostExecutable)) {
    throw "Native Messaging host executable not found: $hostExecutable"
}
foreach ($entry in @(@($chrome, "chrome.json"), @($firefox, "firefox.json"))) {
    $manifestPath = Join-Path $manifestDir $entry[1]
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $manifest.path = $hostExecutable
    [System.IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 8), $utf8NoBom)
    New-Item -Path $entry[0] -Force | Out-Null
    Set-Item -Path $entry[0] -Value $manifestPath
}
Write-Host "Chrome and Firefox Native Messaging hosts registered."
