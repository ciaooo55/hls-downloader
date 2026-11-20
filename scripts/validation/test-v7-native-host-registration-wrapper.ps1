[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$wrapper = Join-Path $repo 'scripts\install\register-v7-native-host.ps1'
$fixtureRoot = Join-Path $repo ('.tool-cache\test-tmp\native-host-wrapper-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null
$engine = Join-Path $fixtureRoot 'GUI engine.exe'
$marker = Join-Path $fixtureRoot 'completed.txt'
$previousMarker = $env:HLS_TEST_REGISTRATION_MARKER
$previousExit = $env:HLS_TEST_REGISTRATION_EXIT
$previousLastExitCode = $global:LASTEXITCODE

try {
    Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Threading;
public class RegistrationFixture {
    public static int Main(string[] args) {
        Thread.Sleep(300);
        File.WriteAllText(Environment.GetEnvironmentVariable("HLS_TEST_REGISTRATION_MARKER"), args[0]);
        return Int32.Parse(Environment.GetEnvironmentVariable("HLS_TEST_REGISTRATION_EXIT"));
    }
}
'@ -OutputAssembly $engine -OutputType WindowsApplication
    $env:HLS_TEST_REGISTRATION_MARKER = $marker
    foreach ($unregister in @($false, $true)) {
        $argument = if ($unregister) { '--unregister-native-host' } else { '--register-native-host' }
        foreach ($exitCode in @(0, 23)) {
            foreach ($staleExitCode in @($null, 91)) {
                Remove-Item -LiteralPath $marker -ErrorAction SilentlyContinue
                $env:HLS_TEST_REGISTRATION_EXIT = [string]$exitCode
                $global:LASTEXITCODE = $staleExitCode
                $failure = ''
                try {
                    & $wrapper -EngineExecutable $engine -Unregister:$unregister
                } catch {
                    $failure = $_.Exception.Message
                }
                if (-not (Test-Path -LiteralPath $marker) -or (Get-Content -LiteralPath $marker -Raw) -ne $argument) {
                    throw "Wrapper did not wait for $argument"
                }
                if ($exitCode -eq 0 -and $failure) { throw "Successful $argument failed: $failure" }
                if ($exitCode -ne 0 -and $failure -ne "v7 Native Host command failed with exit $exitCode") {
                    throw "Wrong failure for ${argument}: $failure"
                }
                Write-Output "PASS $argument exit=$exitCode stale=$staleExitCode"
            }
        }
    }
    $failure = ''
    try { & $wrapper -EngineExecutable (Join-Path $fixtureRoot 'missing.exe') } catch { $failure = $_.Exception.Message }
    if ($failure -notlike 'v7 Engine executable not found.*') { throw "Missing engine was not rejected: $failure" }
    Write-Output 'PASS missing engine rejected'
    Write-Output 'Native Host registration wrapper: 9 cases passed.'
} finally {
    $env:HLS_TEST_REGISTRATION_MARKER = $previousMarker
    $env:HLS_TEST_REGISTRATION_EXIT = $previousExit
    $global:LASTEXITCODE = $previousLastExitCode
    Remove-Item -LiteralPath $fixtureRoot -Recurse -Force
}
