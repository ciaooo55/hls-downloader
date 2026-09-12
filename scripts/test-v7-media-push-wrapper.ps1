[CmdletBinding()]
param(
    [switch]$KeepFixtures,
    [string]$WrapperPath = ''
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$wrapper = if ($WrapperPath) { (Resolve-Path -LiteralPath $WrapperPath).Path } else { Join-Path $PSScriptRoot 'verify-v7-browser-media-push.ps1' }
$fixtureRoot = Join-Path $repo ('artifacts\v7-productization\wrapper-contract-' + [guid]::NewGuid().ToString('n'))
$utf8 = New-Object Text.UTF8Encoding($false)
$previousRunnerTemp = $env:RUNNER_TEMP
$fixtureCommit = (& git -C $repo rev-parse HEAD).Trim()
$fixtureTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
$results = @()

# The fixture repositories inherit this checkout's git identity. No git refs,
# native executables, browser registrations, MSI products or LAN receivers
# are modified. Cleanup requests are recorded, never executed by the wrapper.

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Invoke-Case([string]$Name, [string]$ExpectedError = '') {
    $caseRoot = Join-Path $fixtureRoot $Name
    $scripts = Join-Path $caseRoot 'scripts'
    $candidateRoot = Join-Path $caseRoot 'candidate'
    $runnerTemp = Join-Path $caseRoot 'runner-temp'
    New-Item -ItemType Directory -Path $scripts, $candidateRoot, $runnerTemp -Force | Out-Null
    $fixtureWrapper = Join-Path $scripts 'verify-v7-browser-media-push.ps1'
    Copy-Item -LiteralPath $wrapper -Destination $fixtureWrapper
    $resources = Join-Path $caseRoot 'desktop_ui\resources\common'
    $calledPath = Join-Path $caseRoot 'core-called.txt'
    $zipPath = Join-Path $candidateRoot 'portable.zip'
    $manifestPath = Join-Path $candidateRoot 'ARTIFACT-MANIFEST.json'
    $archiveInput = Join-Path $caseRoot 'archive-input'
    $hostRoot = Join-Path $archiveInput 'HLSDownloader\app\resources'
    New-Item -ItemType Directory -Path $hostRoot -Force | Out-Null
    $hostPath = Join-Path $hostRoot 'HLSDownloaderNativeHost.exe'
    # The .exe fixture is inert text and is never launched.
    $hostHash = ''
    if ($Name -eq 'missing-host') {
        [IO.File]::WriteAllText((Join-Path $hostRoot 'not-the-host.txt'), 'missing host fixture', $utf8)
    } else {
        [IO.File]::WriteAllText($hostPath, 'inert native host fixture', $utf8)
        $hostHash = (Get-FileHash -LiteralPath $hostPath -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    Compress-Archive -LiteralPath (Join-Path $archiveInput 'HLSDownloader') -DestinationPath $zipPath
    if ($Name -eq 'invalid-archive') { [IO.File]::WriteAllText($zipPath, 'not a zip archive', $utf8) }
    $zipHash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $manifest = [ordered]@{
        schema = 1
        package_tier = 'candidate'
        source_commit = $fixtureCommit
        source_tree = $fixtureTree
        artifacts = [ordered]@{ portable = [ordered]@{ path = 'portable.zip'; sha256 = $zipHash } }
    }
    if ($Name -eq 'source-mismatch') { $manifest.source_commit = '3333333333333333333333333333333333333333' }
    if ($Name -eq 'digest-mismatch') { $manifest.artifacts.portable.sha256 = ('0' * 64) }
    [IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 6), $utf8)

    $coreSource = @'
param([string]$CandidateManifestPath, [string]$ExpectedTvboxHost, [string]$InstallDir)
$root = Split-Path $PSScriptRoot -Parent
[IO.File]::WriteAllText((Join-Path $root 'core-called.txt'), 'called')
$mode = Split-Path $root -Leaf
if ($mode -eq 'core-failure') { throw 'injected core verifier failure' }
if ($mode -eq 'missing-summary') { Write-Output 'no summary'; return }
$m = [IO.File]::ReadAllText($CandidateManifestPath) | ConvertFrom-Json
$hostPath = Join-Path $root 'desktop_ui\resources\common\HLSDownloaderNativeHost.exe'
$hash = (Get-FileHash -LiteralPath $hostPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($mode -eq 'host-mismatch') { $hash = ('0' * 64) }
[ordered]@{
    passed = $true
    source_commit = $m.source_commit
    source_tree = $m.source_tree
    native_host_executable = [ordered]@{ sha256 = $hash }
} | ConvertTo-Json -Depth 5 -Compress
'@
    [IO.File]::WriteAllText((Join-Path $scripts 'verify-v7-browser-media-push-core.ps1'), $coreSource, $utf8)

    $sentinel = Join-Path $resources 'keep.txt'
    if ($Name -eq 'existing-directory') {
        New-Item -ItemType Directory -Path $resources -Force | Out-Null
        [IO.File]::WriteAllText($sentinel, 'pre-existing staging', $utf8)
    }
    if ($Name -eq 'existing-file') {
        New-Item -ItemType Directory -Path (Split-Path $resources -Parent) -Force | Out-Null
        $sentinel = $resources
        [IO.File]::WriteAllText($sentinel, 'pre-existing staging', $utf8)
    }
    $cleanupRequests = New-Object 'System.Collections.Generic.List[string]'
    function Remove-Item {
        [CmdletBinding()]
        param([string]$LiteralPath, [switch]$Recurse, [switch]$Force)
        $cleanupRequests.Add($LiteralPath)
    }
    function Expand-Archive {
        param([string]$LiteralPath, [string]$DestinationPath, [switch]$Force)
        Microsoft.PowerShell.Archive\Expand-Archive -LiteralPath $LiteralPath -DestinationPath $DestinationPath -Force:$Force
        if ($Name -eq 'concurrent-directory') {
            New-Item -ItemType Directory -Path $resources -Force | Out-Null
            [IO.File]::WriteAllText((Join-Path $resources 'keep.txt'), 'concurrent staging', $utf8)
        }
    }
    $env:RUNNER_TEMP = $runnerTemp
    $failure = $null
    $output = @()
    try {
        $output = @(& $fixtureWrapper -CandidateManifestPath $manifestPath -ExpectedTvboxHost '192.168.1.2')
    } catch {
        $failure = $_
    }

    if ($Name -eq 'success') {
        Assert-True ($null -eq $failure) "${Name}: unexpected failure: $failure"
        $summary = ($output | Select-Object -Last 1) | ConvertFrom-Json
        Assert-True ($summary.passed -eq $true) 'Success fixture did not pass.'
        Assert-True ($summary.native_host_provenance.candidate_portable_sha256 -eq $zipHash) 'Portable digest was not preserved.'
        Assert-True ($summary.native_host_provenance.portable_native_host_sha256 -eq $hostHash) 'Host provenance was not preserved.'
    } else {
        Assert-True ($null -ne $failure) "${Name}: expected a terminating failure."
        if ($ExpectedError) {
            Assert-True ($failure.ToString() -match $ExpectedError) "${Name}: wrong failure: $failure"
        }
        Assert-True (@($output | Where-Object { $_ -match '"passed":true' }).Count -eq 0) "${Name}: emitted false success."
    }
    if ($Name -in @('existing-directory', 'existing-file')) {
        Assert-True (Test-Path -LiteralPath $sentinel -PathType Leaf) "${Name}: pre-existing staging was deleted."
        Assert-True ([IO.File]::ReadAllText($sentinel) -eq 'pre-existing staging') "${Name}: pre-existing staging was modified."
    } elseif ($Name -eq 'concurrent-directory') {
        Assert-True (Test-Path -LiteralPath $sentinel -PathType Leaf) 'Concurrent staging was deleted.'
        Assert-True ([IO.File]::ReadAllText($sentinel) -eq 'concurrent staging') 'Concurrent staging was modified.'
    }
    $shouldCallCore = $Name -in @('success', 'core-failure', 'missing-summary', 'host-mismatch')
    Assert-True ((Test-Path -LiteralPath $calledPath) -eq $shouldCallCore) "${Name}: unexpected core invocation state."
    $stagingCleanups = @($cleanupRequests | Where-Object { $_ -eq $resources })
    Assert-True ($stagingCleanups.Count -eq [int]$shouldCallCore) "${Name}: cleanup targeted unowned staging or omitted owned staging."
    $shouldCleanTemp = $Name -notin @('existing-directory', 'existing-file', 'source-mismatch', 'digest-mismatch')
    $tempCleanups = @($cleanupRequests | Where-Object { (Split-Path $_ -Parent) -eq $runnerTemp })
    Assert-True ($tempCleanups.Count -eq [int]$shouldCleanTemp) "${Name}: unexpected extraction cleanup requests."
    foreach ($path in $tempCleanups) {
        Assert-True (Test-Path -LiteralPath $path -PathType Container) "${Name}: cleanup targeted an uncreated extraction directory."
    }
    Assert-True ($cleanupRequests.Count -eq ($stagingCleanups.Count + $tempCleanups.Count)) "${Name}: cleanup escaped owned paths."
    return [ordered]@{ name = $Name; passed = $true }
}

New-Item -ItemType Directory -Path $fixtureRoot -ErrorAction Stop | Out-Null
try {
    $results += Invoke-Case 'existing-directory' 'staging unexpectedly survived'
    $results += Invoke-Case 'existing-file' 'staging unexpectedly survived'
    $results += Invoke-Case 'concurrent-directory'
    $results += Invoke-Case 'invalid-archive'
    $results += Invoke-Case 'missing-host' 'missing HLSDownloaderNativeHost.exe'
    $results += Invoke-Case 'core-failure' 'injected core verifier failure'
    $results += Invoke-Case 'missing-summary' 'did not emit its JSON summary'
    $results += Invoke-Case 'host-mismatch' 'not bound to the manifest-bound'
    $results += Invoke-Case 'source-mismatch' 'current source commit/tree'
    $results += Invoke-Case 'digest-mismatch' 'SHA-256 mismatch'
    $results += Invoke-Case 'success'
    Write-Output ([ordered]@{
        schema = 1
        passed = $true
        kind = 'synthetic_wrapper_contract_only'
        powershell = $PSVersionTable.PSVersion.ToString()
        cases = $results
        count = $results.Count
    } | ConvertTo-Json -Depth 6 -Compress)
} finally {
    $env:RUNNER_TEMP = $previousRunnerTemp
    if (-not $KeepFixtures) {
        Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction Stop
    }
}
