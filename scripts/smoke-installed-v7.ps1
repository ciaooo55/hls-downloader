[CmdletBinding()]
param(
    [string]$InstallDir = '',
    [int]$Port = 19744,
    [string]$Token = 'v7-installed-ui-api-20260824',
    [string]$ReportPath = ''
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$programsRoot = [IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs')).TrimEnd('\', '/')
$root = if ([String]::IsNullOrWhiteSpace($InstallDir)) {
    Join-Path $programsRoot 'HLSDownloader'
} else {
    [IO.Path]::GetFullPath($InstallDir).TrimEnd('\', '/')
}
if (-not $root.StartsWith(($programsRoot + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) {
    throw "Installed v7 smoke target must stay under ${programsRoot}: $root"
}
$executable = Join-Path $root 'HLSDownloader.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Installed v7 executable is missing: $executable"
}
if ($Token.Length -lt 16) {
    throw 'Installed v7 smoke token must contain at least 16 characters.'
}

$env:HLS_UI_TEST_API = '1'
$env:HLS_UI_TEST_TOKEN = $Token
$env:HLS_UI_TEST_PORT = [string]$Port
$env:HLS_UI_AUDIT_WIDTH = '1280'
$env:HLS_UI_AUDIT_HEIGHT = '760'
$headers = @{ 'X-HLS-Test-Token' = $Token }
$launcher = $null
try {
    $existing = Invoke-RestMethod -Uri "http://127.0.0.1:${Port}/health" -Headers $headers -TimeoutSec 2
} catch {
    $existing = $null
}
if (-not $existing) {
    $launcher = Start-Process -FilePath $executable -WorkingDirectory $root -PassThru
}
$deadline = (Get-Date).AddSeconds(90)
$health = $null
do {
    Start-Sleep -Milliseconds 250
    try {
        $health = Invoke-RestMethod -Uri "http://127.0.0.1:${Port}/health" -Headers $headers -TimeoutSec 2
    } catch {
        $health = $null
    }
} while (-not $health -and (Get-Date) -lt $deadline)
if (-not $health) {
    throw 'Installed v7 UI API did not become ready.'
}
if ($health.version -ne '7.0.0' -or -not $health.ok) {
    throw "Installed v7 health response is invalid: $($health | ConvertTo-Json -Compress)"
}
$windowDeadline = (Get-Date).AddSeconds(30)
do {
    $window = Invoke-RestMethod -Uri "http://127.0.0.1:${Port}/window" -Headers $headers -TimeoutSec 5
    if ($window.showing) { break }
    Start-Sleep -Milliseconds 200
} while ((Get-Date) -lt $windowDeadline)
if (-not $window.showing -or $window.width -ne 1280 -or $window.height -ne 760 -or $window.iconCount -lt 1) {
    throw "Installed v7 window state is invalid: $($window | ConvertTo-Json -Compress)"
}

$processes = @(Get-CimInstance Win32_Process | Where-Object {
    ($_.ExecutablePath -and $_.ExecutablePath.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) -or
    ($_.CommandLine -and $_.CommandLine.IndexOf($root, [StringComparison]::OrdinalIgnoreCase) -ge 0)
} | ForEach-Object {
    [ordered]@{
        pid = [int]$_.ProcessId
        name = [string]$_.Name
        executable = [string]$_.ExecutablePath
    }
})
if (-not ($processes | Where-Object { $_.name -eq 'HLSDownloader.exe' })) {
    throw 'Installed Compose workbench process was not found.'
}
if (-not ($processes | Where-Object { $_.name -eq 'HLSDownloaderEngine.exe' })) {
    throw 'Installed Rust Engine process was not found.'
}
if (-not ($processes | Where-Object { $_.name -eq 'HLSDownloaderPresenter.exe' })) {
    throw 'Installed hot Presenter process was not found.'
}

$report = [ordered]@{
    schema = 1
    passed = $true
    version = [string]$health.version
    install_dir = $root
    launcher_pid = if ($launcher) { [int]$launcher.Id } else { 0 }
    api = "http://127.0.0.1:${Port}"
    window = $window
    processes = $processes
}
$output = if ([String]::IsNullOrWhiteSpace($ReportPath)) {
    Join-Path $repo 'artifacts\v7-productization\installed\smoke.json'
} else {
    [IO.Path]::GetFullPath($ReportPath)
}
New-Item -ItemType Directory -Force -Path (Split-Path $output -Parent) | Out-Null
[IO.File]::WriteAllText($output, ($report | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
$report | ConvertTo-Json -Depth 6
