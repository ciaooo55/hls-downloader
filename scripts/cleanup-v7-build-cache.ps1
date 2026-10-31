[CmdletBinding(SupportsShouldProcess)]
param([switch]$Apply)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path.TrimEnd('\', '/')
$cacheRoot = Join-Path $repo '.tool-cache\build-cache'
$relativeTargets = @(
    'target', 'native_shell\target', 'presenter_ui\target',
    'desktop_ui\build', 'desktop_ui\resources\common',
    'extension\.output', 'extension\.wxt',
    '.tool-cache\build-cache\cargo-target', '.tool-cache\build-cache\compose-build'
)
if ($env:HLS_V7_BUILD_CACHE) {
    $cacheRoot = [IO.Path]::GetFullPath($env:HLS_V7_BUILD_CACHE)
    if (-not $cacheRoot.StartsWith($repo + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Build cache must remain inside this project workspace.'
    }
}
$targets = @($relativeTargets | ForEach-Object { Join-Path $repo $_ }) + @(
    (Join-Path $cacheRoot 'cargo-target'), (Join-Path $cacheRoot 'compose-build')
)
$targets = @($targets | Sort-Object -Unique | Where-Object { Test-Path -LiteralPath $_ })
foreach ($target in $targets) {
    $resolved = [IO.Path]::GetFullPath($target)
    if (-not $resolved.StartsWith($repo + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Cleanup target escaped workspace: $resolved"
    }
    $cursor = Get-Item -LiteralPath $resolved -Force
    while ($cursor.FullName -ne $repo) {
        if ($cursor.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Cleanup refuses a linked directory: $($cursor.FullName)"
        }
        $cursor = $cursor.Parent
    }
}
if (-not $Apply) {
    $targets | ForEach-Object { Write-Output "DRY-RUN: $_" }
    return
}
$processes = @(Get-CimInstance Win32_Process)
foreach ($target in $targets) {
    $running = @($processes | Where-Object {
        $_.ProcessId -ne $PID -and (
            ($_.ExecutablePath -and $_.ExecutablePath.StartsWith($target + '\', [StringComparison]::OrdinalIgnoreCase)) -or
            ($_.CommandLine -and $_.CommandLine.IndexOf($target, [StringComparison]::OrdinalIgnoreCase) -ge 0)
        )
    })
    if ($running.Count) { throw "Build output is in use: $target (PIDs $($running.ProcessId -join ', '))" }
}
foreach ($target in $targets) {
    if ($PSCmdlet.ShouldProcess($target, 'Remove rebuildable output')) {
        $pending = New-Object 'Collections.Generic.Stack[IO.DirectoryInfo]'
        $pending.Push((Get-Item -LiteralPath $target -Force))
        while ($pending.Count) {
            foreach ($child in $pending.Pop().GetFileSystemInfos()) {
                if ($child.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                    throw "Cleanup refuses a linked output entry: $($child.FullName)"
                }
                if ($child -is [IO.DirectoryInfo]) { $pending.Push($child) }
            }
        }
        Remove-Item -LiteralPath $target -Recurse -Force
    }
}
