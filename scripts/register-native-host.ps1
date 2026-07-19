param([switch]$Unregister)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$name = "com.ciaooo55.hls_downloader"
$chrome = "HKCU:\Software\Google\Chrome\NativeMessagingHosts\$name"
$firefox = "HKCU:\Software\Mozilla\NativeMessagingHosts\$name"
if ($Unregister) {
    Remove-Item -LiteralPath $chrome, $firefox -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "Browser integration removed."
    exit 0
}
$manifestDir = Join-Path $root "native-host"
foreach ($entry in @(@($chrome, "chrome.json"), @($firefox, "firefox.json"))) {
    New-Item -Path $entry[0] -Force | Out-Null
    Set-Item -Path $entry[0] -Value (Join-Path $manifestDir $entry[1])
}
Write-Host "Chrome and Firefox Native Messaging hosts registered."
