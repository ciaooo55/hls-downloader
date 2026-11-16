#Requires -Version 7.0
param(
    [Parameter(Mandatory)][string]$TestExe,
    [Parameter(Mandatory)][string]$OutputDir,
    [ValidateRange(1, 20)][int]$Runs = 5
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$exe = (Resolve-Path -LiteralPath $TestExe).Path
$output = [IO.Path]::GetFullPath($OutputDir, $repo)
if (-not $output.StartsWith($repo + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'OutputDir must stay inside the project workspace.'
}
New-Item -ItemType Directory -Force -Path $output | Out-Null
$temporary = Join-Path $repo '.tool-cache/test-tmp/idm-streaming-20261116'
New-Item -ItemType Directory -Force -Path $temporary | Out-Null
$results = @()
for ($run = 1; $run -le $Runs; $run++) {
    $info = [Diagnostics.ProcessStartInfo]::new($exe)
    $info.WorkingDirectory = $repo
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.Environment['TEMP'] = $temporary
    $info.Environment['TMP'] = $temporary
    foreach ($argument in @('media::hls::tests::hls_segment_transfer_benchmark', '--exact', '--ignored', '--nocapture', '--test-threads=1')) {
        $info.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::Start($info)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $peakWorkingSet = 0L
        $peakPagedMemory = 0L
        $cpuSeconds = 0.0
        while (-not $process.WaitForExit(5)) {
            $process.Refresh()
            $peakWorkingSet = [Math]::Max($peakWorkingSet, $process.PeakWorkingSet64)
            $peakPagedMemory = [Math]::Max($peakPagedMemory, $process.PeakPagedMemorySize64)
            $cpuSeconds = [Math]::Max($cpuSeconds, $process.TotalProcessorTime.TotalSeconds)
            if ($watch.Elapsed.TotalSeconds -gt 60) {
                $process.Kill($true)
                $process.WaitForExit()
                throw "Benchmark run $run exceeded 60 seconds."
            }
        }
        $cpuSeconds = [Math]::Max($cpuSeconds, $process.TotalProcessorTime.TotalSeconds)
        $text = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
        [IO.File]::WriteAllText((Join-Path $output "run-$run.log"), $text)
        if ($process.ExitCode -ne 0 -or $text -notmatch 'HLS_SEGMENT_BENCH bytes=(\d+) seconds=([\d.]+) mib_per_sec=([\d.]+)') {
            throw "Benchmark run $run failed; see its log."
        }
        $results += [pscustomobject]@{
            run = $run
            bytes = [long]$Matches[1]
            seconds = [double]::Parse($Matches[2], [Globalization.CultureInfo]::InvariantCulture)
            mib_per_sec = [double]::Parse($Matches[3], [Globalization.CultureInfo]::InvariantCulture)
            cpu_seconds = $cpuSeconds
            peak_working_set_bytes = $peakWorkingSet
            peak_paged_memory_bytes = $peakPagedMemory
        }
    } finally {
        $process.Dispose()
    }
}
$medians = [ordered]@{}
foreach ($name in @('seconds', 'mib_per_sec', 'cpu_seconds', 'peak_working_set_bytes', 'peak_paged_memory_bytes')) {
    $sorted = @($results.$name | Sort-Object)
    $middle = [int][Math]::Floor($sorted.Count / 2)
    $medians[$name] = if ($sorted.Count % 2) { $sorted[$middle] } else { ($sorted[$middle - 1] + $sorted[$middle]) / 2 }
}
$report = [ordered]@{
    executable = $exe
    sha256 = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    scenario = '32 MiB clear HLS segment over loopback TCP; exact saved bytes checked; no browser/UI/IDM'
    runs = $results
    median = $medians
    limits = 'Development test binary; process CPU/peak memory include fixture and byte validation. Not a real-media, HTTPS, concurrent or IDM comparison.'
}
$report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $output 'benchmark.json') -Encoding utf8
$report.median | ConvertTo-Json
