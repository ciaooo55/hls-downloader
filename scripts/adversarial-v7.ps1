[CmdletBinding()]
param(
    [ValidateSet('native', 'browser', 'transfer')]
    [string[]]$Scope = @('native')
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path
$presenterManifest = Get-Content -LiteralPath (Join-Path $repo 'presenter_ui\Cargo.toml') -Raw -Encoding UTF8
$presenterSource = Get-Content -LiteralPath (Join-Path $repo 'presenter_ui\src\hot_main.rs') -Raw -Encoding UTF8
$presenterBuild = Get-Content -LiteralPath (Join-Path $repo 'presenter_ui\build.rs') -Raw -Encoding UTF8
$presenterUi = Get-Content -LiteralPath (Join-Path $repo 'presenter_ui\ui\hot.slint') -Raw -Encoding UTF8
if ($presenterManifest -notmatch 'autobins\s*=\s*false' -or
    $presenterManifest -notmatch 'path\s*=\s*"src/hot_main.rs"' -or
    $presenterSource -match '\b(MainWindow|SettingsWindow|PlayerWindow)\b' -or
    $presenterBuild -notmatch 'ui/hot\.slint' -or
    $presenterUi -match '\b(MainWindow|SettingsWindow|PlayerWindow|LegalWindow|NewTaskWindow)\b' -or
    ([regex]::Matches($presenterUi, 'export component\s+\w+Window')).Count -ne 3) {
    throw 'v7 presenter architecture gate failed: only the dedicated hot-window entry may be active.'
}
# Project build outputs stay inside the repository.
$cacheRoot = Join-Path $repo '.tool-cache\build-cache'
$cargoCommand = Get-Command cargo.exe -ErrorAction SilentlyContinue
$cargo = if ($cargoCommand) { $cargoCommand.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
$env:CARGO_HOME = Join-Path $cacheRoot 'cargo'
$env:CARGO_TARGET_DIR = Join-Path $cacheRoot 'cargo-target'
$env:GRADLE_USER_HOME = Join-Path $cacheRoot 'gradle'
$jdkRoot = $env:HLS_V7_JAVA_HOME
if(-not $jdkRoot -and (Test-Path (Join-Path $cacheRoot 'jdk-21\bin\java.exe'))){ $jdkRoot = Join-Path $cacheRoot 'jdk-21' }
# Legacy read-only tool location from earlier installs; tools are not project content.
if(-not $jdkRoot -and (Test-Path 'E:\HLSDownloaderBuildCache\jdk-21\bin\java.exe')){ $jdkRoot = 'E:\HLSDownloaderBuildCache\jdk-21' }
if(-not $jdkRoot){ throw 'JDK 21 was not found. Set HLS_V7_JAVA_HOME or run scripts\bootstrap-v7-toolchain.ps1.' }
$env:JAVA_HOME = $jdkRoot
$pythonCommand = Get-Command python.exe -ErrorAction SilentlyContinue
$pythonExe = if ($env:HLS_V7_PYTHON) { $env:HLS_V7_PYTHON } elseif ($pythonCommand) { $pythonCommand.Source } else { 'C:\Users\lee\.conda\envs\test\python.exe' }
$reportDirectory = Join-Path $repo 'artifacts\v7-implementation\adversarial'
$reportPath = Join-Path $reportDirectory 'latest.json'
$completedGates = [System.Collections.Generic.List[string]]::new()
$result = 'failed'
$failure = ''

function Invoke-Gate {
    param([string]$Name, [scriptblock]$Action)
    Write-Host "[adversarial] $Name"
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "Adversarial gate failed: $Name (exit $LASTEXITCODE)"
    }
    [void]$script:completedGates.Add($Name)
}

function Assert-ArchitectureBoundary {
    $composeSources = Join-Path $repo 'desktop_ui\src'
    $forbidden = Get-ChildItem -LiteralPath $composeSources -Recurse -File -Filter '*.kt' |
        Select-String -Pattern 'jdbc:sqlite|DriverManager|rusqlite' -CaseSensitive:$false
    if ($forbidden) {
        throw "Compose must not access SQLite directly: $($forbidden[0].Path):$($forbidden[0].LineNumber)"
    }
    $nativeHostSource = Join-Path $repo 'native_shell\src\native_host.rs'
    if (-not (Select-String -LiteralPath $nativeHostSource -Pattern 'connect_or_start_core' -Quiet)) {
        throw 'Native Host cold-start boundary is missing.'
    }
}

Push-Location $repo
try {
    Assert-ArchitectureBoundary
    Invoke-Gate 'Rust Core protocol and state invariants' {
        & $cargo test --manifest-path native_shell\Cargo.toml --lib --no-default-features --quiet
    }
    Invoke-Gate 'v7 Rust transfer worker contract' {
        & $cargo test --manifest-path native_shell\Cargo.toml --lib --no-default-features http_engine --quiet
        if ($LASTEXITCODE -ne 0) { throw "v7 Rust transfer worker tests failed (exit $LASTEXITCODE)" }
    }
    Invoke-Gate 'Native UI task model invariants' {
        & $cargo test --manifest-path presenter_ui\Cargo.toml --quiet
        if ($LASTEXITCODE -ne 0) { throw "Native UI tests failed (exit $LASTEXITCODE)" }
        $presenter = Join-Path $env:CARGO_TARGET_DIR 'debug\hls-downloader-presenter.exe'
        if (-not (Test-Path -LiteralPath $presenter)) {
            & $cargo build --manifest-path presenter_ui\Cargo.toml --bin hls-downloader-presenter --quiet
            if ($LASTEXITCODE -ne 0) { throw "v7 presenter build failed (exit $LASTEXITCODE)" }
        }
        if (-not (Test-Path -LiteralPath $presenter)) { throw "v7 presenter missing after build: $presenter" }
        $selfTest = & $presenter --self-test 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0 -or $selfTest -notmatch 'hls-v7-presenter/1 ok') {
            throw "v7 presenter self-test failed: $selfTest"
        }
        $engine = Join-Path $env:CARGO_TARGET_DIR 'debug\hls-downloader-engine.exe'
        if (-not (Test-Path -LiteralPath $engine)) {
            & $cargo build --manifest-path native_shell\Cargo.toml --bin hls-downloader-engine --quiet
            if ($LASTEXITCODE -ne 0) { throw "v7 engine build failed (exit $LASTEXITCODE)" }
        }
        if (-not (Test-Path -LiteralPath $engine)) { throw "v7 engine missing after build: $engine" }
        $nativeHost = Join-Path $env:CARGO_TARGET_DIR 'debug\HLSDownloaderNativeHost.exe'
        if (-not (Test-Path -LiteralPath $nativeHost)) {
            & $cargo build --manifest-path native_shell\Cargo.toml --bin HLSDownloaderNativeHost --quiet
            if ($LASTEXITCODE -ne 0) { throw "v7 Native Host build failed (exit $LASTEXITCODE)" }
        }
        if (-not (Test-Path -LiteralPath $nativeHost)) { throw "v7 Native Host missing after build: $nativeHost" }
        & $pythonExe scripts\smoke_v7_presenter.py `
            --presenter $presenter --host $nativeHost --engine $engine
        if ($LASTEXITCODE -ne 0) { throw "v7 hot presenter smoke failed (exit $LASTEXITCODE)" }
        & $pythonExe scripts\smoke_v7_player_process.py --engine $engine
        if ($LASTEXITCODE -ne 0) { throw "v7 player process smoke failed (exit $LASTEXITCODE)" }
    }
    Invoke-Gate 'Compose protocol and hostile-input invariants' {
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\build-v7.ps1 -Task test
    }
    Invoke-Gate 'Native Host cold-start and framed-message boundary' {
        & $pythonExe scripts\smoke_v7_native_host.py `
            --host "$env:CARGO_TARGET_DIR\debug\HLSDownloaderNativeHost.exe" `
            --engine "$env:CARGO_TARGET_DIR\debug\hls-downloader-engine.exe" `
            --report "$repo\artifacts\v7-productization\performance\native-host-cold-start.json"
    }

    Invoke-Gate 'Real Engine Range throughput, memory and publication boundary' {
        & $cargo build --release --manifest-path native_shell\Cargo.toml --bin hls-downloader-engine --quiet
        if ($LASTEXITCODE -ne 0) { throw "v7 release Engine build failed (exit $LASTEXITCODE)" }
        & $pythonExe scripts\smoke_v7_transfer_performance.py `
            --engine "$env:CARGO_TARGET_DIR\release\hls-downloader-engine.exe" `
            --report "$repo\artifacts\v7-productization\performance\real-transfer-latest.json"
    }

    if ($Scope -contains 'browser') {
        Invoke-Gate 'Browser extension Native Messaging contract' {
            Push-Location extension
            try { & pnpm test } finally { Pop-Location }
        }
    }
    if ($Scope -contains 'transfer') {
        Invoke-Gate 'Rust transfer recovery and media harness' {
            & $cargo test --manifest-path native_shell\Cargo.toml --lib http_engine --quiet
            if ($LASTEXITCODE -ne 0) { throw "HTTP transfer tests failed (exit $LASTEXITCODE)" }
            & $cargo test --manifest-path native_shell\Cargo.toml --lib media::harness --quiet
            if ($LASTEXITCODE -ne 0) { throw "media harness tests failed (exit $LASTEXITCODE)" }
        }
    }
    $result = 'passed'
    Write-Host "[adversarial] PASS scopes: $($Scope -join ', ')"
} catch {
    $failure = $_.Exception.Message
    throw
} finally {
    New-Item -ItemType Directory -Force -Path $reportDirectory | Out-Null
    $report = [ordered]@{
        schema = 1
        finished_at = [DateTime]::UtcNow.ToString('o')
        status = $result
        scopes = @($Scope)
        completed_gates = @($completedGates)
        failure = $failure
    } | ConvertTo-Json -Depth 4
    [IO.File]::WriteAllText($reportPath, $report, [Text.UTF8Encoding]::new($false))
    Pop-Location
}
