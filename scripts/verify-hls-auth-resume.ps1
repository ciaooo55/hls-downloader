[CmdletBinding()]
param(
    [string]$ReportPath = 'artifacts\v7-productization\hls-auth-resume-evidence.txt',
    [string]$CargoPath = '',
    [ValidateRange(1, 20)]
    [int]$Runs = 3
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$manifest = Join-Path $root 'native_shell\Cargo.toml'
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

if ([String]::IsNullOrWhiteSpace($CargoPath)) {
    $cargoCandidate = Join-Path $HOME '.cargo\bin\cargo.exe'
    if (Test-Path -LiteralPath $cargoCandidate -PathType Leaf) {
        $CargoPath = $cargoCandidate
    } else {
        $cargoCommand = Get-Command cargo.exe -ErrorAction SilentlyContinue
        if ($null -eq $cargoCommand) {
            throw 'cargo.exe was not found. Pass -CargoPath or install Rust.'
        }
        $CargoPath = $cargoCommand.Source
    }
}
$CargoPath = [IO.Path]::GetFullPath($CargoPath)
if (-not (Test-Path -LiteralPath $CargoPath -PathType Leaf)) {
    throw "cargo.exe was not found: $CargoPath"
}

$filters = @(
    'media::hls::tests::authenticated_vod_pause_resume_reuses_completed_segments',
    'media::hls::tests::authenticated_live_pause_resume_restores_atomic_timeline'
)
$report = New-Object System.Collections.Generic.List[string]
$report.Add('HLS authenticated VOD/Live pause-resume verification')
$report.Add("UTC date: $([DateTime]::UtcNow.ToString('o'))")
$report.Add("Manifest: $manifest")
$report.Add("Cargo: $CargoPath")
$report.Add("Runs: $Runs")
$overall = 0

for ($run = 1; $run -le $Runs; $run++) {
    foreach ($filter in $filters) {
        $arguments = @('test', '--manifest-path', $manifest, '--lib', $filter, '--', '--exact', '--nocapture')
        $command = '"{0}" {1}' -f $CargoPath, ($arguments -join ' ')
        $report.Add('')
        $report.Add("RUN: $run")
        $report.Add("COMMAND: $command")
        $lines = New-Object 'System.Collections.Generic.List[string]'
        $previousErrorActionPreference = $ErrorActionPreference
        try {
            # Windows PowerShell 5.1 surfaces native stderr warnings as ErrorRecord objects.
            $ErrorActionPreference = 'Continue'
            & $CargoPath @arguments 2>&1 | ForEach-Object {
                [void]$lines.Add([string]$_)
                [void]$report.Add([string]$_)
            }
        } finally {
            $ErrorActionPreference = $previousErrorActionPreference
        }
        $status = $LASTEXITCODE
        $report.Add("EXIT_STATUS: $status")
        $output = $lines.ToArray() -join "`n"
        $testName = ($filter -split '::')[-1]
        $testPattern = 'test\s+' + [regex]::Escape($filter) + '\s+\.\.\.\s+ok'
        $resultPattern = 'test result: ok\.\s+1 passed;\s+0 failed'
        if ($status -ne 0) {
            $overall = $status
        } elseif ($output -notmatch $testPattern -or $output -notmatch $resultPattern) {
            $report.Add("ASSERTION: missing exact passing result for $testName run $run")
            $overall = 1
        }
    }
}

$report.Add('')
if ($overall -eq 0) {
    $report.Add('RESULT: authenticated VOD and Live pause-resume verification passed')
} else {
    $report.Add("RESULT: verification failed with exit status $overall")
}

$reportFullPath = if ([IO.Path]::IsPathRooted($ReportPath)) {
    [IO.Path]::GetFullPath($ReportPath)
} else {
    [IO.Path]::GetFullPath((Join-Path $root $ReportPath))
}
$parent = Split-Path -Parent $reportFullPath
if (-not (Test-Path -LiteralPath $parent)) {
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
}
[System.IO.File]::WriteAllText($reportFullPath, ($report -join [Environment]::NewLine), $utf8NoBom)
if ($overall -ne 0) {
    exit $overall
}
