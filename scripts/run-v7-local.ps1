[CmdletBinding()]
param(
    [ValidateSet('native', 'compose')][string]$UiRuntime = 'native'
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path "$PSScriptRoot\..").Path

Push-Location $repo
try {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\build-v7.ps1 -Task run -UiRuntime $UiRuntime
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
    Pop-Location
}
