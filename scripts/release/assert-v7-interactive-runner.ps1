[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

if ($env:OS -ne 'Windows_NT') {
    throw 'Installed-browser media-push validation requires Windows.'
}

$current = Get-Process -Id $PID -ErrorAction Stop
$sessionId = [int]$current.SessionId
if ($sessionId -le 0) {
    throw 'Installed-browser media-push validation requires an interactive logged-in Windows session; Session 0 service runners are not supported.'
}
if (-not [Environment]::UserInteractive) {
    throw "Installed-browser media-push validation requires UserInteractive=true in Windows session $sessionId."
}

$desktopShell = @(Get-Process -Name explorer -ErrorAction SilentlyContinue | Where-Object {
    [int]$_.SessionId -eq $sessionId
})
if ($desktopShell.Count -eq 0) {
    throw "Installed-browser media-push validation requires an Explorer desktop in the current Windows session $sessionId. Start the hls-release runner from the logged-in release user's desktop, not as a Windows service."
}

# Callers may also inspect LASTEXITCODE after invoking this PowerShell script.
# A successful .ps1 invocation does not inherently reset a stale native-process
# exit code, so normalize it explicitly; failures throw before reaching here.
$global:LASTEXITCODE = 0
Write-Output ([ordered]@{
    schema = 1
    passed = $true
    session_id = $sessionId
    user_interactive = [Environment]::UserInteractive
    explorer_processes = @($desktopShell | ForEach-Object { [int]$_.Id })
} | ConvertTo-Json -Compress)
