param(
    [switch]$SkipUiBuild
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$Vcvars = Join-Path $PSScriptRoot "vcvars.ps1"
if (Test-Path -LiteralPath $Vcvars) {
    . $Vcvars
}

Push-Location $Root
try {
    cargo test --manifest-path (Join-Path $Root "native_shell\Cargo.toml") --locked --lib --no-default-features
    if ($LASTEXITCODE -ne 0) { throw "native_shell v6 Core tests failed" }
    cargo test --manifest-path (Join-Path $Root "native_shell\Cargo.toml") --locked
    if ($LASTEXITCODE -ne 0) { throw "native_shell tests failed" }
    cargo test --manifest-path (Join-Path $Root "native_ui\Cargo.toml") --locked
    if ($LASTEXITCODE -ne 0) { throw "native_ui tests failed" }
    if (-not $SkipUiBuild) {
        cargo build --manifest-path (Join-Path $Root "native_ui\Cargo.toml") --locked --bin HLSDownloader
        if ($LASTEXITCODE -ne 0) { throw "native_ui build failed" }
    }
    Write-Host "v6 cargo gates passed. GitHub Release ships Core+Slint and pinned libmpv-2.dll."
} finally {
    Pop-Location
}
