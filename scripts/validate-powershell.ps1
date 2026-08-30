param(
    [string[]]$Path = @(
        "scripts\adversarial-v7.ps1",
        "scripts\benchmark-v7.ps1",
        "scripts\bootstrap-v7-toolchain.ps1",
        "scripts\build-v7.ps1",
        "scripts\cleanup-v7-build-cache.ps1",
        "scripts\cleanup-v7-legacy-install.ps1",
        "scripts\create-v7-portable.ps1",
        "scripts\install-v7-local.ps1",
        "scripts\register-v7-native-host.ps1",
        "scripts\record-v7-release-gate.ps1",
        "scripts\run-v7-local.ps1",
        "scripts\set-v7-msi-rollback-order.ps1",
        "scripts\vcvars.ps1",
        "scripts\shutdown-running.ps1",
        "scripts\upgrade-v7-portable.ps1",
        "scripts\smoke-v7-portable-upgrade.ps1",
        "scripts\smoke-installed-v7.ps1",
        "scripts\verify-v7-feature-parity.ps1",
        "scripts\verify-hls-auth-resume.ps1",
        "scripts\verify-v7-bt-selection.ps1",
        "scripts\smoke-v7-compose-frames.ps1",
        "scripts\verify-hls-candidate-auth-resume.ps1",
        "scripts\validate-powershell.ps1"
    )
)

$ErrorActionPreference = "Stop"
$failed = $false
foreach ($item in $Path) {
    $resolved = (Resolve-Path -LiteralPath $item).Path
    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile(
        $resolved,
        [ref]$tokens,
        [ref]$errors
    ) | Out-Null
    foreach ($parseError in @($errors)) {
        $failed = $true
        Write-Error "$item`: $($parseError.Message)"
    }
}
if ($failed) {
    exit 1
}
Write-Output "PowerShell syntax validation passed for $($Path.Count) scripts."
