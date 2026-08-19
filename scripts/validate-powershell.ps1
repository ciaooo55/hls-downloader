param(
    [string[]]$Path = @(
        "scripts\build_installer.ps1",
        "scripts\build_v6.ps1",
        "scripts\register-native-host.ps1",
        "scripts\run_v6_gates.ps1",
        "scripts\vcvars.ps1",
        "scripts\shutdown-running.ps1",
        "scripts\smoke-installer-upgrade.ps1",
        "scripts\smoke-portable-upgrade.ps1",
        "scripts\upgrade-portable.ps1"
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
