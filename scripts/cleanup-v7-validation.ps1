[CmdletBinding(SupportsShouldProcess)]
param([switch]$Apply)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..')).TrimEnd('\')
$evidence = Join-Path $repo 'artifacts\desktop-click-20261102'
$plan = Get-Content -LiteralPath (Join-Path $evidence 'cleanup-plan.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader'
if ($plan.schema -ne 1 -or $plan.workspace -ne $repo -or $plan.desktop_directory -ne $desktop) {
    throw 'Cleanup plan belongs to a different workspace or desktop directory.'
}

function Assert-SafePath([string]$Path, [string]$Root) {
    $absolute = [IO.Path]::GetFullPath($Path)
    if (-not $absolute.StartsWith($Root + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Path escaped its allowed directory: $absolute"
    }
    $cursor = Get-Item -LiteralPath $absolute -Force
    while ($null -ne $cursor) {
        if ($cursor.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Linked paths are not allowed: $($cursor.FullName)"
        }
        if ($cursor.FullName -eq $Root) { return }
        if ($cursor -is [IO.DirectoryInfo]) { $cursor = $cursor.Parent }
        else { $cursor = $cursor.Directory }
    }
    throw "Allowed directory not found in path ancestry: $absolute"
}

function Get-Entries([string]$Path) {
    $item = Get-Item -LiteralPath $Path -Force
    $entries = New-Object 'Collections.Generic.List[object]'
    $pending = New-Object 'Collections.Generic.Stack[IO.FileSystemInfo]'
    $pending.Push($item)
    while ($pending.Count) {
        $entry = $pending.Pop()
        if ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Linked entries are not allowed: $($entry.FullName)"
        }
        $relative = '.'
        if ($entry.FullName -ne $item.FullName) {
            $relative = $entry.FullName.Substring($item.FullName.Length + 1).Replace('\', '/')
        }
        if ($entry -is [IO.DirectoryInfo]) {
            $entries.Add([pscustomobject]@{path=$relative; kind='directory'; bytes=0; sha256=''; full_path=$entry.FullName})
            foreach ($child in $entry.GetFileSystemInfos()) { $pending.Push($child) }
        }
        else {
            $entries.Add([pscustomobject]@{
                path=$relative; kind='file'; bytes=$entry.Length
                sha256=(Get-FileHash -LiteralPath $entry.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
                full_path=$entry.FullName
            })
        }
    }
    $entries | Sort-Object path
}

# Only these inspected leftovers are eligible; the plan cannot add other targets.
$allowed = @(
    'artifacts\desktop-click-20261102\msi-inspection',
    'artifacts\desktop-click-20261102\previous-extensions',
    '.tool-cache\test-tmp\desktop-click-package',
    'artifacts\popup-bootstrap-20261101\original',
    'artifacts\popup-bootstrap-20261101\fixed',
    'artifacts\popup-bootstrap-20261101\powershell-5',
    'artifacts\popup-bootstrap-20261101\powershell-7',
    'artifacts\popup-bootstrap-20261101\repack-input',
    '.tool-cache\test-tmp\popup-bootstrap-build',
    'extension\.output',
    'artifacts\desktop-click-20261102\verify-package.py'
)
$oldMsiName = 'HLSDownloader-7.0.3-20261101-' + [char]0x672A + [char]0x7B7E + [char]0x540D + '.msi'
$expectedTargets = @($allowed | ForEach-Object { 'workspace:' + $_ }) + @('desktop:' + $oldMsiName)
$actualTargets = @($plan.targets | ForEach-Object { $_.root + ':' + $_.path })
if (@(Compare-Object $expectedTargets $actualTargets).Count -ne 0 -or $actualTargets.Count -ne $expectedTargets.Count) {
    throw 'Cleanup plan does not match the fixed target list.'
}

# The retained delivery and evidence must be intact before removing any old copy.
foreach ($required in $plan.required_files) {
    if ($required.root -eq 'workspace') { $rootPath = $repo }
    elseif ($required.root -eq 'desktop') { $rootPath = $desktop }
    else { throw 'Invalid required file root.' }
    $path = Join-Path $rootPath $required.path
    Assert-SafePath $path $rootPath
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $required.sha256) {
        throw "Retained delivery or evidence changed: $path"
    }
}

$processes = @(Get-CimInstance Win32_Process)
$checked = New-Object 'Collections.Generic.List[object]'
foreach ($target in $plan.targets) {
    if ($target.root -eq 'workspace') { $rootPath = $repo } else { $rootPath = $desktop }
    $path = Join-Path $rootPath $target.path
    if (-not (Test-Path -LiteralPath $path)) {
        $checked.Add([pscustomobject]@{path=$path; status='already absent'; files=0; bytes=0; entries=@()})
        continue
    }
    Assert-SafePath $path $rootPath
    $entries = @(Get-Entries $path)
    $expected = @($target.entries | Sort-Object path)
    if ($entries.Count -ne $expected.Count) { throw "Target entries changed: $path" }
    for ($i = 0; $i -lt $entries.Count; $i++) {
        foreach ($property in @('path', 'kind', 'bytes', 'sha256')) {
            if ($entries[$i].$property -cne $expected[$i].$property) {
                throw "Target changed; nothing will be deleted: $path [$($entries[$i].path), $property]"
            }
        }
    }
    $running = @($processes | Where-Object {
        $_.ProcessId -ne $PID -and (
            ($_.ExecutablePath -and ($_.ExecutablePath -eq $path -or $_.ExecutablePath.StartsWith($path + '\', [StringComparison]::OrdinalIgnoreCase))) -or
            ($_.CommandLine -and $_.CommandLine.IndexOf($path, [StringComparison]::OrdinalIgnoreCase) -ge 0)
        )
    })
    if ($running.Count) { throw "Cleanup target is in use: $path (PIDs $($running.ProcessId -join ', '))" }
    $files = @($entries | Where-Object { $_.kind -eq 'file' })
    $checked.Add([pscustomobject]@{path=$path; status='verified'; files=$files.Count; bytes=($files | Measure-Object bytes -Sum).Sum; entries=$entries})
}

$checked | Select-Object status, files, @{Name='MiB';Expression={[math]::Round($_.bytes / 1MB, 2)}}, path | Format-Table -AutoSize
$total = ($checked | Measure-Object bytes -Sum).Sum
Write-Host ('Verified cleanup size: {0:N2} MiB. Other files are retained.' -f ($total / 1MB))
if (-not $Apply) {
    Write-Host 'Preview only. Run this script with -Apply to delete the verified leftovers.'
    return
}

# Check all file locks before starting deletion. Never stop another process.
if (-not $WhatIfPreference) {
    foreach ($target in $checked) {
        foreach ($entry in $target.entries) {
            if ($entry.kind -ne 'file') { continue }
            $handle = [IO.File]::Open($entry.full_path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
            $handle.Dispose()
        }
    }
}
$failure = $null
try {
    foreach ($target in $checked) {
        if ($target.status -eq 'already absent') { continue }
        if ($PSCmdlet.ShouldProcess($target.path, 'Delete inspected validation leftovers')) {
            Assert-SafePath $target.path $(if ($target.path.StartsWith($repo + '\', [StringComparison]::OrdinalIgnoreCase)) { $repo } else { $desktop })
            Remove-Item -LiteralPath $target.path -Recurse -Force
            $target.status = 'removed'
        }
        else { $target.status = 'not removed' }
    }
}
catch { $failure = $_.Exception.Message; throw }
finally {
    if (-not $WhatIfPreference) {
        [ordered]@{
            executed_at_utc=[DateTime]::UtcNow.ToString('o'); error=$failure
            results=@($checked | Select-Object path,status,files,bytes)
        } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $evidence 'cleanup-result.json') -Encoding UTF8
    }
}
