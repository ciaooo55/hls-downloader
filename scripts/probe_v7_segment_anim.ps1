# Probe: does the sidebar segment/navigation selection state really ANIMATE?
#
# Why this exists: segmentBackground()/rememberPressFeedback drive selection
# colours with a 120-130ms tween. A before/after screenshot only shows the two
# end states and cannot tell a tween from an instant colour swap. The same
# applies to anything reading only "/state".
#
# Method: a JVM sampler grabs the sidebar strip with Robot.createScreenCapture
# every ~12ms. The click POST is sent from inside that sampler, so sampling is
# already running while the animation plays. Judgment is on the pixel column of
# the old selected row, the new selected row, and an inert row:
#
#   PASS requires
#     1. old row background has >=1 frame strictly between first and last value
#        (a jump has zero such frames);
#     2. inert row keeps exactly one distinct value for the whole capture;
#     3. the click itself succeeded.
#
# PASS means the selection transition is a real animation; condition 2 stops an
# "everything changed" screenshot from being read as success.
#
# ASCII-only on purpose: Windows PowerShell 5.1 reads BOM-less files as GBK.
#
# Usage:
#   pwsh -NoProfile -File scripts/probe_v7_segment_anim.ps1 -AppPath <HLSDownloader.exe> [-Out <dir>]

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$AppPath,
    [string]$JdkHome = $(if ($env:JAVA_HOME) { $env:JAVA_HOME } else { 'A:\hls-build-cache\jdk-21' }),
    [int]$Port = 19741,
    [string]$Token = 'hls-seg-anim-20260923',
    [int]$Tx = 100,
    [int]$Ty = 188,
    [int]$YOld = 141,
    [int]$YNew = 188,
    [int]$YInert = 400,
    [int]$Left = 0,
    [int]$Top = 0,
    [string]$Fixture = 'tasks_1000',
    [string]$Theme = 'dark',
    [int]$Width = 1400,
    [int]$Height = 820,
    [string]$Out = 'A:\hls-build-cache\logs\anim-probe\run',
    [string]$BuildCache = 'A:\hls-build-cache'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$probeDir = Join-Path $PSScriptRoot 'probe'
$src = Join-Path $probeDir 'ProbeSegmentAnim.java'
$javac = Join-Path $JdkHome 'bin\javac.exe'
$java = Join-Path $JdkHome 'bin\java.exe'
if (-not (Test-Path -LiteralPath $javac)) { throw "javac not found at $javac (pass -JdkHome)" }

$AppPath = [IO.Path]::GetFullPath($AppPath)
$appDir = [IO.Path]::GetDirectoryName($AppPath)

$productNames = @('HLSDownloader.exe', 'HLSDownloaderEngine.exe', 'HLSDownloaderPresenter.exe', 'HLSDownloaderNativeHost.exe')
$initialIds = @(Get-CimInstance Win32_Process | Where-Object { $_.Name -in $productNames } | ForEach-Object { $_.ProcessId })

New-Item -ItemType Directory -Force -Path $Out | Out-Null
Get-ChildItem -LiteralPath $Out -Filter '*.png' -ErrorAction SilentlyContinue | Remove-Item -Force

& $javac -d $probeDir $src
if ($LASTEXITCODE -ne 0) { throw 'javac failed' }

$header = @{ 'X-HLS-Test-Token' = $Token }
$base = "http://127.0.0.1:$Port"

$env:HLS_V7_BUILD_CACHE = $BuildCache
$env:JAVA_HOME = $JdkHome
$env:HLS_V6_SKIP_MIGRATE = '1'
$env:HLS_UI_TEST_API = '1'
$env:HLS_UI_TEST_TOKEN = $Token
$env:HLS_UI_TEST_PORT = [string]$Port
$env:HLS_UI_AUDIT_SURFACE = $Fixture
$env:HLS_UI_AUDIT_THEME = $Theme
$env:HLS_UI_AUDIT_WIDTH = [string]$Width
$env:HLS_UI_AUDIT_HEIGHT = [string]$Height
$env:HLS_V7_DATA_DIR = Join-Path $BuildCache 'logs\anim-probe\profile'

$runner = $null
try {
    $runner = Start-Process -FilePath $AppPath -WorkingDirectory $appDir -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $PSScriptRoot 'probe-run-out.log') `
        -RedirectStandardError (Join-Path $PSScriptRoot 'probe-run-err.log') -PassThru

    $ready = $false
    for ($i = 0; $i -lt 90; $i++) {
        try {
            $r = Invoke-WebRequest -Uri "$base/health" -Headers $header -TimeoutSec 3 -UseBasicParsing
            if ($r.StatusCode -eq 200) { $ready = $true; break }
        } catch { }
        Start-Sleep -Milliseconds 500
    }
    if (-not $ready) {
        $err = if (Test-Path (Join-Path $PSScriptRoot 'probe-run-err.log')) {
            [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'probe-run-err.log'), [Text.Encoding]::UTF8) } else { '' }
        throw "UI test API never became healthy. $err"
    }

    Invoke-WebRequest -Uri "$base/action" -Method Post -Headers $header `
        -Body (@{ type = 'activate' } | ConvertTo-Json -Compress) `
        -ContentType 'application/json' -TimeoutSec 30 -UseBasicParsing | Out-Null
    Start-Sleep -Milliseconds 900

    $sum = & $java -cp $probeDir ProbeSegmentAnim $Out $base $Token $Tx $Ty $YOld $YNew $YInert $Left $Top
    [IO.File]::WriteAllText((Join-Path $Out 'probe-info.json'), $sum, (New-Object Text.UTF8Encoding($false)))
    Write-Host $sum

    $info = $sum | ConvertFrom-Json
    if ($info.clickReply -notmatch 'ok') { throw "click failed: $($info.clickReply)" }

    $samples = Import-Csv -LiteralPath (Join-Path $Out 'samples.csv')
    # samples.csv rows are single-pixel ARGB hex; split channels before judging.
    $O = @($samples | ForEach-Object { [Convert]::ToInt32($_.old.Substring(8), 16) })
    $N = @($samples | ForEach-Object { [Convert]::ToInt32($_.new.Substring(8), 16) })
    $K = @($samples | ForEach-Object { [Convert]::ToInt32($_.inert.Substring(8), 16) })

    function Count-Mid([int[]]$vals) {
        return @($vals | Where-Object { $_ -ne $vals[0] -and $_ -ne $vals[-1] }).Count
    }
    $midR = Count-Mid @($O | ForEach-Object { ($_ -shr 16) -band 0xFF })
    $midG = Count-Mid @($O | ForEach-Object { ($_ -shr 8) -band 0xFF })
    $midB = Count-Mid @($O | ForEach-Object { $_ -band 0xFF })
    $inertDistinct = @($K | Sort-Object -Unique).Count

    $fail = @()
    if ($inertDistinct -gt 1) { $fail += "inert row changed across $inertDistinct values" }
    if ($midR -lt 1 -or $midG -lt 1 -or $midB -lt 1) { $fail += "old row jumped: R=$midR G=$midG B=$midB mid frames" }

    $verdict = [pscustomobject]@{
        sampleCount = $info.sampleCount
        savedFrames = @($info.savedList).Count
        clickReplyMs = $info.clickReplyMs
        oldFirst = "0x$('{0:X8}' -f $O[0])"
        oldLast = "0x$('{0:X8}' -f $O[-1])"
        oldMidRed = $midR
        oldMidGreen = $midG
        oldMidBlue = $midB
        inertDistinct = $inertDistinct
        savedList = $info.savedList
        verdict = if ($fail.Count -eq 0) { 'PASS' } else { 'FAIL' }
        reasons = $fail
    } | ConvertTo-Json -Compress
    [IO.File]::WriteAllText((Join-Path $Out 'verdict.json'), $verdict, (New-Object Text.UTF8Encoding($false)))
    Write-Host $verdict
    if ($fail.Count -gt 0) { exit 1 }
    exit 0
} finally {
    if ($runner -and -not $runner.HasExited) {
        & taskkill.exe /PID $runner.Id /T /F 2>$null | Out-Null
        Wait-Process -Id $runner.Id -Timeout 5 -ErrorAction SilentlyContinue
    }
    foreach ($proc in @(Get-CimInstance Win32_Process | Where-Object {
        $_.Name -in $productNames -and $_.ProcessId -notin $initialIds
    })) {
        Stop-Process -Id $proc.ProcessId -Force -ErrorAction SilentlyContinue
    }
}
