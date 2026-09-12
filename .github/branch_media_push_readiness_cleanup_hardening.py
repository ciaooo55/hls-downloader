from pathlib import Path

path = Path("scripts/verify-v7-browser-media-push-core.ps1")
text = path.read_text(encoding="utf-8")

old = '''$result = $null
$expectedNativeHost = [IO.Path]::GetFullPath((Join-Path $InstallDir 'app\\resources\\HLSDownloaderNativeHost.exe'))
'''
new = '''$result = $null
$primaryFailure = $null
$cleanupFailures = New-Object 'System.Collections.Generic.List[string]'
$expectedNativeHost = [IO.Path]::GetFullPath((Join-Path $InstallDir 'app\\resources\\HLSDownloaderNativeHost.exe'))
'''
if old not in text:
    raise SystemExit("result-state anchor missing")
text = text.replace(old, new, 1)

old = '''        $reportPath = Join-Path $evidenceRoot "$browser-tvbox-real.json"
        $arguments = @(
'''
new = '''        $reportPath = Join-Path $evidenceRoot "$browser-tvbox-real.json"
        if (Test-Path -LiteralPath $reportPath) {
            Remove-Item -LiteralPath $reportPath -Force -ErrorAction Stop
        }
        $arguments = @(
'''
if old not in text:
    raise SystemExit("report preflight anchor missing")
text = text.replace(old, new, 1)

old = '''        $captured = @(& $pythonExe @arguments 2>&1 | ForEach-Object { $_.ToString() })
        if ($LASTEXITCODE -ne 0) { throw "$browser installed-browser TVBox smoke failed: $($captured -join [Environment]::NewLine)" }
        $report = [IO.File]::ReadAllText($reportPath, $utf8NoBom) | ConvertFrom-Json
'''
new = '''        $captured = @(& $pythonExe @arguments 2>&1 | ForEach-Object { $_.ToString() })
        if ($LASTEXITCODE -ne 0) { throw "$browser installed-browser TVBox smoke failed: $($captured -join [Environment]::NewLine)" }
        if (-not (Test-Path -LiteralPath $reportPath -PathType Leaf)) {
            throw "$browser installed-browser TVBox smoke did not create a fresh report: $reportPath"
        }
        $report = [IO.File]::ReadAllText($reportPath, $utf8NoBom) | ConvertFrom-Json
'''
if old not in text:
    raise SystemExit("report freshness anchor missing")
text = text.replace(old, new, 1)

old = '''    Write-Output ($result | ConvertTo-Json -Depth 8 -Compress)
} finally {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'shutdown-running.ps1') -InstallDir $InstallDir | Out-Null
    if ($installed) {
        [void](Invoke-Msi @('/x', $candidate, '/qn') $uninstallLog)
    }
'''
new = '''} catch {
    $primaryFailure = $_
} finally {
    try {
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'shutdown-running.ps1') -InstallDir $InstallDir | Out-Null
        if ($LASTEXITCODE -ne 0) {
            $cleanupFailures.Add("shutdown-running.ps1 exited with $LASTEXITCODE")
        }
    } catch {
        $cleanupFailures.Add("shutdown-running.ps1 failed: $($_.Exception.Message)")
    }
    if ($installed) {
        try {
            $uninstallExit = Invoke-Msi @('/x', $candidate, '/qn') $uninstallLog
            if ($uninstallExit -notin @(0, 3010, 1641)) {
                $cleanupFailures.Add("candidate MSI uninstall failed with exit $uninstallExit")
            }
        } catch {
            $cleanupFailures.Add("candidate MSI uninstall threw: $($_.Exception.Message)")
        }
    }
    try {
        if ($null -ne (Get-InstalledProduct $upgradeCode)) {
            $cleanupFailures.Add('candidate MSI product registration survived cleanup')
        }
    } catch {
        $cleanupFailures.Add("failed to verify MSI product cleanup: $($_.Exception.Message)")
    }
    if (Test-Path -LiteralPath $expectedNativeHost -PathType Leaf) {
        $cleanupFailures.Add("installed Native Messaging host survived cleanup: $expectedNativeHost")
    }
    try {
        $postEdgeRegistration = @(Get-NativeHostRegistration 'HKCU:\\Software\\Microsoft\\Edge\\NativeMessagingHosts')
        if ($postEdgeRegistration.Count -gt 0) {
            $cleanupFailures.Add('Edge Native Messaging registration survived cleanup')
        }
    } catch {
        $cleanupFailures.Add("failed to verify Edge registration cleanup: $($_.Exception.Message)")
    }
    try {
        $postFirefoxRegistration = @(Get-NativeHostRegistration 'HKCU:\\Software\\Mozilla\\NativeMessagingHosts')
        if ($postFirefoxRegistration.Count -gt 0) {
            $cleanupFailures.Add('Firefox Native Messaging registration survived cleanup')
        }
    } catch {
        $cleanupFailures.Add("failed to verify Firefox registration cleanup: $($_.Exception.Message)")
    }
'''
if old not in text:
    raise SystemExit("cleanup anchor missing")
text = text.replace(old, new, 1)

old = '''    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
'''
new = '''    try {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction Stop
    } catch {
        $cleanupFailures.Add("temporary release-gate state survived cleanup: $($_.Exception.Message)")
    }
}

if ($null -ne $primaryFailure) {
    if ($cleanupFailures.Count -gt 0) {
        throw "Browser media-push validation failed: $($primaryFailure.Exception.Message); cleanup also failed: $($cleanupFailures -join '; ')"
    }
    throw $primaryFailure
}
if ($cleanupFailures.Count -gt 0) {
    throw "Browser media-push cleanup failed after validation: $($cleanupFailures -join '; ')"
}
if ($null -eq $result) {
    throw 'Browser media-push validation completed without a result.'
}
Write-Output ($result | ConvertTo-Json -Depth 8 -Compress)
'''
if old not in text:
    raise SystemExit("final-output anchor missing")
text = text.replace(old, new, 1)

path.write_text(text, encoding="utf-8")
