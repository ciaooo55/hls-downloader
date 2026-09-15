[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$AppPath,
    [string]$OutputDir = '',
    [string[]]$Fixtures = @('tasks_1000'),
    [string[]]$Themes = @('light', 'dark'),
    [string[]]$Sizes = @('1400x820'),
    [string]$Token = 'hls-visual-capture-20260914',
    [int]$Port = 19739,
    [int]$TimeoutSeconds = 120,
    [int]$SettleMilliseconds = 2500,
    [switch]$ParkPointer,
    [int]$ParkPointerX = 4,
    [int]$ParkPointerY = 4
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $repo 'artifacts\v7-productization\compose-visual'
}
$OutputDir = [IO.Path]::GetFullPath($OutputDir)
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$logDir = Join-Path $OutputDir 'logs'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

$AppPath = [IO.Path]::GetFullPath($AppPath)
if (-not (Test-Path -LiteralPath $AppPath -PathType Leaf)) { throw "Compose app is missing: $AppPath" }

if ($Token.Length -lt 16) { throw 'Token must contain at least 16 characters.' }

# The workbench refuses to start a second instance; fail loudly instead of
# silently screenshotting somebody else's window.
$running = @(Get-CimInstance Win32_Process | Where-Object {
    $_.Name -eq 'HLSDownloader.exe' -or
    ($_.Name -eq 'java.exe' -and $_.CommandLine -like '*com.hlsdownloader.desktop.MainKt*')
})
if ($running.Count -gt 0) {
    throw "A HLS Downloader workbench is already running (PID $($running.ProcessId -join ',')). Close it before capturing."
}

$appDir = [IO.Path]::GetDirectoryName($AppPath)
$dataDir = Join-Path $OutputDir 'profile'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null

$header = @{ 'X-HLS-Test-Token' = $Token }
$base = "http://127.0.0.1:$Port"
$results = @()
$productProcessNames = @('HLSDownloader.exe', 'HLSDownloaderEngine.exe', 'HLSDownloaderPresenter.exe', 'HLSDownloaderNativeHost.exe')
$ParkPointerActive = [bool]$ParkPointer

# 夹具截图走的是 Robot 的**屏幕截取**（/screenshot 截的是 window.locationOnScreen 那块矩形），
# 所以真实指针停在哪里会渗进截图：指针压在某个控件上，那个控件就带着悬停底色被拍下来。
# 实测过两次：基线里 cast-dark 的侧栏某行带了约 10% 黑的悬停态层，而同轮其它夹具在
# 同一点上没有；改后那轮 tasks_1000-light 里又有一行带上了 surface2 悬停底。
# 两次都不是产品差异，是"拍的时候指针在哪儿"的差异。
#
# 归位（把指针先移到一个惰性点再截）是正确方向，但**归位点本身必须先被验证**：
#   * 同一坐标重复到达要逐像素可复现；
#   * 该点底下不能有任何可悬停控件。
# 我试过 (4,4)，结果它反而让改后那轮比"不归位"偏离基线更多 —— 说明 (4,4) 并不能
# 真正清掉悬停态（怀疑它落在标题栏里，Compose 收不到悬停更新，于是保留了启动时指针
# 造成的状态）。所以这个开关**默认关闭**：未经证实的采集改动不该默认生效，
# 否则就会把"环境噪声"写进证据里，而噪声比不做更糟。
#
# 要启用它，先用 scripts/calibrate_move_action.py 标定出一个可复现的惰性点，
# 再用 -ParkPointer -ParkPointerX/-ParkPointerY 显式打开。
# 坐标是**窗口相对**的，/action 会拒绝越界坐标，所以只能停在窗口内部。
function Invoke-ParkPointer {
    param([string]$Base, [hashtable]$Header, [int]$X, [int]$Y)
    $body = @{ type = 'move'; x = $X; y = $Y } | ConvertTo-Json -Compress
    try {
        Invoke-WebRequest -Uri "$Base/action" -Method Post -Headers $Header -Body $body `
            -ContentType 'application/json' -TimeoutSec 10 -UseBasicParsing | Out-Null
        return $true
    } catch {
        return $false
    }
}

function Wait-Health {
    param([int]$Seconds)
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $r = Invoke-WebRequest -Uri "$base/health" -Headers $header -TimeoutSec 3 -UseBasicParsing
            if ($r.StatusCode -eq 200) { return $r.Content }
        } catch { }
        Start-Sleep -Milliseconds 300
    }
    return $null
}

foreach ($size in $Sizes) {
    $parts = $size -split 'x'
    if ($parts.Count -ne 2) { throw "Invalid size '$size'; expected WIDTHxHEIGHT." }
    $width = [int]$parts[0]
    $height = [int]$parts[1]
    if ($width -lt 1024 -or $height -lt 600) { throw "Size '$size' is below the 1024x600 workbench minimum." }

    foreach ($theme in $Themes) {
        foreach ($fixture in $Fixtures) {
            $name = "$fixture-$theme-${width}x${height}"
            $stdoutPath = Join-Path $logDir "$name.stdout.log"
            $stderrPath = Join-Path $logDir "$name.stderr.log"
            $pngPath = Join-Path $OutputDir "$name.png"

            $env:HLS_UI_TEST_API = '1'
            $env:HLS_UI_TEST_TOKEN = $Token
            $env:HLS_UI_TEST_PORT = [string]$Port
            $env:HLS_UI_AUDIT_SURFACE = $fixture
            $env:HLS_UI_AUDIT_THEME = $theme
            $env:HLS_UI_AUDIT_WIDTH = [string]$width
            $env:HLS_UI_AUDIT_HEIGHT = [string]$height
            $env:HLS_V6_SKIP_MIGRATE = '1'
            $env:HLS_V7_DATA_DIR = $dataDir

            $runner = $null
            $status = 'failed'
            $detail = ''
            try {
                $runner = Start-Process -FilePath $AppPath -WorkingDirectory $appDir -PassThru `
                    -WindowStyle Hidden -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
                $health = Wait-Health -Seconds $TimeoutSeconds
                if (-not $health) {
                    $err = if (Test-Path -LiteralPath $stderrPath) { [IO.File]::ReadAllText($stderrPath, [Text.Encoding]::UTF8) } else { '' }
                    throw "test API did not become healthy within $TimeoutSeconds seconds. $err"
                }
                if ($ParkPointerActive) {
                    if (-not (Invoke-ParkPointer -Base $base -Header $header -X $ParkPointerX -Y $ParkPointerY)) {
                        throw "could not park the pointer before capture; hover state would leak into this fixture"
                    }
                }
                Start-Sleep -Milliseconds $SettleMilliseconds
                Invoke-WebRequest -Uri "$base/screenshot" -Headers $header -TimeoutSec 30 -OutFile $pngPath -UseBasicParsing | Out-Null
                $window = (Invoke-WebRequest -Uri "$base/window" -Headers $header -TimeoutSec 10 -UseBasicParsing).Content
                $bytes = (Get-Item -LiteralPath $pngPath).Length
                if ($bytes -lt 4096) { throw "screenshot is implausibly small ($bytes bytes)" }
                $status = 'captured'
                $detail = "$window"
            } catch {
                $detail = $_.Exception.Message
            } finally {
                if ($runner -and -not $runner.HasExited) {
                    & taskkill.exe /PID $runner.Id /T /F 2>$null | Out-Null
                    Wait-Process -Id $runner.Id -Timeout 5 -ErrorAction SilentlyContinue
                }
                foreach ($proc in @(Get-CimInstance Win32_Process | Where-Object { $_.Name -in $productProcessNames })) {
                    Stop-Process -Id $proc.ProcessId -Force -ErrorAction SilentlyContinue
                }
                Start-Sleep -Milliseconds 800
            }

            $results += [pscustomobject]@{
                fixture = $fixture
                theme   = $theme
                width   = $width
                height  = $height
                status  = $status
                png     = if ($status -eq 'captured') { $pngPath } else { '' }
                bytes   = if ($status -eq 'captured') { (Get-Item -LiteralPath $pngPath).Length } else { 0 }
                detail  = $detail
            }
            Write-Host ("{0,-46} {1}" -f $name, $status)
        }
    }
}

$reportPath = Join-Path $OutputDir 'capture-report.json'
$passed = @($results | Where-Object { $_.status -eq 'captured' }).Count
$summary = [pscustomobject]@{
    schema   = 1
    app_path = $AppPath
    captured = $passed
    total    = $results.Count
    results  = $results
}
[IO.File]::WriteAllText($reportPath, ($summary | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding($false)))
Write-Host ($summary | ConvertTo-Json -Depth 2 -Compress)
if ($passed -ne $results.Count) { exit 1 }
