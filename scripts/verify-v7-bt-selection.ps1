[CmdletBinding()]
param(
    [ValidateRange(1, 20)]
    [int]$Runs = 3,
    [string]$CargoPath = ''
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

if ([String]::IsNullOrWhiteSpace($CargoPath)) {
    $cargoCandidate = Join-Path $HOME '.cargo\bin\cargo.exe'
    if (Test-Path -LiteralPath $cargoCandidate -PathType Leaf) {
        $CargoPath = $cargoCandidate
    } else {
        $cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
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

function Invoke-CargoTest([string]$TestPath) {
    $arguments = @(
        'test',
        '--manifest-path', (Join-Path $repo 'native_shell\Cargo.toml'),
        '--lib',
        $TestPath,
        '--',
        '--exact',
        '--nocapture'
    )
    $lines = New-Object 'System.Collections.Generic.List[string]'
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        # Windows PowerShell 5.1 surfaces native stderr warnings as errors.
        $ErrorActionPreference = 'Continue'
        & $CargoPath @arguments 2>&1 | ForEach-Object {
            [void]$lines.Add([string]$_)
        }
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    $exitCode = $LASTEXITCODE
    $output = $lines.ToArray() -join "`n"
    if ($exitCode -ne 0) {
        throw "cargo test failed for $TestPath (exit $exitCode):`n$output"
    }
    $testPattern = 'test\s+' + [regex]::Escape($TestPath) + '\s+\.\.\.\s+ok'
    if ($output -notmatch $testPattern -or $output -notmatch 'test result: ok\.\s+1 passed;\s+0 failed') {
        throw "cargo test did not report the expected passing result for ${TestPath}:`n$output"
    }
    return $exitCode
}

$cancelTest = 'download_worker::tests::live_torrent_selection_update_cancels_requested_file_and_publishes_remaining_file'
$resumeTest = 'torrent_engine::tests::multifile_swarm_resumes_without_refetching_and_materializes_selection'
for ($run = 1; $run -le $Runs; $run++) {
    $cancelExit = Invoke-CargoTest $cancelTest
    Write-Output "BT_SELECTION_CANCEL_RUN=$run EXIT=$cancelExit RESULT=cancelled_deselected_file_other_file_completed"
}
$resumeExit = Invoke-CargoTest $resumeTest
Write-Output "BT_SELECTION_RESUME_RUN=1 EXIT=$resumeExit RESULT=missing_pieces_reused_selected_files_materialized"
Write-Output "BT_SELECTION_EVIDENCE=PASS RUNS=$Runs"
