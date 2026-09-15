# Probe: does clicking a collapsed-sidebar rail item actually change the filter?
#
# Why this exists: the rail replaces text labels with icons. A screenshot proves it
# LOOKS right (width, contrast, badges) but not that the 40dp hit boxes land where
# the icons are drawn. The test API's /state now exposes `activeFilter`, so this can
# be asserted directly instead of inferred from "the selection count dropped".
#
# ASCII-only on purpose: Windows PowerShell 5.1 reads BOM-less files as GBK, so CJK
# literals here would need a BOM.
#
# Usage:
#   powershell -File rail-click-probe.ps1 -AppPath <HLSDownloader.exe> [-Fixture tasks_1000]
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$AppPath,
    [string]$Fixture = 'tasks_1000',
    [string]$Theme = 'dark',
    [int]$Width = 1024,
    [int]$Height = 600,
    [int]$RailX = 28,
    [int]$StartY = 80,
    [int]$EndY = 580,
    [int]$StepY = 20,
    [string]$Token = 'hls-visual-capture-20260914',
    [int]$Port = 19739,
    [int]$TimeoutSeconds = 120,
    [int]$SettleMilliseconds = 2500,
    [string]$ReportPath = ''
)

$ErrorActionPreference = 'Stop'

$AppPath = [IO.Path]::GetFullPath($AppPath)
if (-not (Test-Path -LiteralPath $AppPath -PathType Leaf)) { throw "app is missing: $AppPath" }

$running = @(Get-CimInstance Win32_Process | Where-Object {
    $_.Name -eq 'HLSDownloader.exe' -or
    ($_.Name -eq 'java.exe' -and $_.CommandLine -like '*com.hlsdownloader.desktop.MainKt*')
})
if ($running.Count -gt 0) {
    throw "a workbench is already running (PID $($running.ProcessId -join ',')). Close it first."
}

$appDir = [IO.Path]::GetDirectoryName($AppPath)
$header = @{ 'X-HLS-Test-Token' = $Token }
$base = "http://127.0.0.1:$Port"
$productProcessNames = @('HLSDownloader.exe', 'HLSDownloaderEngine.exe', 'HLSDownloaderPresenter.exe', 'HLSDownloaderNativeHost.exe')
$tmp = Join-Path $env:TEMP 'rail-click-probe'
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

function Wait-Health {
    param([int]$Seconds)
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $r = Invoke-WebRequest -Uri "$base/health" -Headers $header -TimeoutSec 3 -UseBasicParsing
            if ($r.StatusCode -eq 200) { return $true }
        } catch { }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Get-Filter {
    $s = (Invoke-WebRequest -Uri "$base/state" -Headers $header -TimeoutSec 10 -UseBasicParsing).Content
    return ($s | ConvertFrom-Json)
}

function Format-State {
    param($st)
    $cat = if ([string]::IsNullOrEmpty($st.activeCategory)) { '-' } else { $st.activeCategory }
    $q = if ([string]::IsNullOrEmpty($st.activeQueueId)) { '-' } else { $st.activeQueueId }
    return ("filter={0,-4} category={1,-4} queue={2}" -f $st.activeFilter, $cat, $q)
}

function Click-At {
    param([int]$X, [int]$Y)
    $body = @{ type = 'click'; x = $X; y = $Y } | ConvertTo-Json -Compress
    Invoke-WebRequest -Uri "$base/action" -Method Post -Headers $header -Body $body `
        -ContentType 'application/json' -TimeoutSec 10 -UseBasicParsing | Out-Null
}

$env:HLS_UI_TEST_API = '1'
$env:HLS_UI_TEST_TOKEN = $Token
$env:HLS_UI_TEST_PORT = [string]$Port
$env:HLS_UI_AUDIT_SURFACE = $Fixture
$env:HLS_UI_AUDIT_THEME = $Theme
$env:HLS_UI_AUDIT_WIDTH = [string]$Width
$env:HLS_UI_AUDIT_HEIGHT = [string]$Height
$env:HLS_V6_SKIP_MIGRATE = '1'
$env:HLS_V7_DATA_DIR = (Join-Path $tmp 'profile')

$runner = $null
$rows = @()
$windowJson = ''
try {
    $runner = Start-Process -FilePath $AppPath -WorkingDirectory $appDir -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $tmp 'stdout.log') -RedirectStandardError (Join-Path $tmp 'stderr.log')
    if (-not (Wait-Health -Seconds $TimeoutSeconds)) {
        $err = if (Test-Path (Join-Path $tmp 'stderr.log')) { [IO.File]::ReadAllText((Join-Path $tmp 'stderr.log'), [Text.Encoding]::UTF8) } else { '' }
        throw "test API never became healthy. $err"
    }
    Start-Sleep -Milliseconds $SettleMilliseconds

    $windowJson = (Invoke-WebRequest -Uri "$base/window" -Headers $header -TimeoutSec 10 -UseBasicParsing).Content
    $initial = Get-Filter
    Write-Host "window : $windowJson"
    Write-Host ("initial: {0}" -f (Format-State $initial))
    Write-Host ''

    # Focus is the flaky part. Observed once: a whole scan returned "no change" for
    # every y because the clicks landed on whatever window was in front. /action
    # wraps each dispatch in withFocusedWindow (toFront + requestFocus + 100ms), but
    # that is apparently not always enough. Activate explicitly, and -- more
    # importantly -- treat an all-unchanged scan as INVALID below rather than as a
    # legitimate "nothing happened" result.
    Invoke-WebRequest -Uri "$base/action" -Method Post -Headers $header `
        -Body (@{ type = 'activate' } | ConvertTo-Json -Compress) `
        -ContentType 'application/json' -TimeoutSec 10 -UseBasicParsing | Out-Null
    Start-Sleep -Milliseconds 400

    for ($y = $StartY; $y -le $EndY; $y += $StepY) {
        Click-At -X $RailX -Y $y
        Start-Sleep -Milliseconds 220
        $st = Get-Filter
        $rows += [pscustomobject]@{
            y = $y; filter = $st.activeFilter
            category = $st.activeCategory; queue = $st.activeQueueId
        }
        Write-Host ("  y={0,4}  ->  {1}" -f $y, (Format-State $st))
    }

    # distinct values in the order they were first seen
    $seen = New-Object System.Collections.Generic.List[string]
    foreach ($r in $rows) {
        $key = "$($r.filter)|$($r.category)|$($r.queue)"
        if (-not $seen.Contains($key)) { $seen.Add($key) }
    }
    Write-Host ''
    Write-Host ("distinct states reached: {0}" -f ($seen -join '  ;  '))

    # Hard failure on an all-unchanged scan. Without this, a run where the clicks
    # never landed is indistinguishable from a legitimate result -- the same class of
    # silent evidence corruption as the off-screen black screenshot.
    if ($seen.Count -le 1) {
        throw "INVALID RUN: every probe returned the same state ($($seen[0])). " +
              "The clicks almost certainly never landed (focus/foreground). Do not read this as a result."
    }
    if ($rows[-1].filter -eq $initial.activeFilter -and $seen.Count -le 2) {
        Write-Warning "only one state transition observed; check focus before trusting the tail of the scan"
    }
} finally {
    if ($runner -and -not $runner.HasExited) {
        & taskkill.exe /PID $runner.Id /T /F 2>$null | Out-Null
        Wait-Process -Id $runner.Id -Timeout 5 -ErrorAction SilentlyContinue
    }
    foreach ($proc in @(Get-CimInstance Win32_Process | Where-Object { $_.Name -in $productProcessNames })) {
        Stop-Process -Id $proc.ProcessId -Force -ErrorAction SilentlyContinue
    }
}

if ($ReportPath) {
    $payload = [pscustomobject]@{
        fixture = $Fixture; theme = $Theme; width = $Width; height = $Height
        railX = $RailX; startY = $StartY; endY = $EndY; stepY = $StepY
        window = $windowJson
        probes = $rows
    }
    $json = $payload | ConvertTo-Json -Depth 5
    [IO.File]::WriteAllText($ReportPath, $json, (New-Object Text.UTF8Encoding($false)))
    Write-Host "written $ReportPath"
}
