[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Shell
)

# Regression test for the release-script hygiene contract enforced by
# validate-powershell.ps1: a release script must never fall back to a
# developer-specific Windows user profile path.
#
# The contract used to be a build-v7.ps1-only check in ci.yml, so two scripts
# carried a personal conda interpreter fallback under a user profile and passed
# CI. The guard now scans every script, and this test keeps it from being narrowed
# again: a clean fixture must pass and an offending fixture must fail with a
# reason, not just a non-zero exit.
#
# Note that this file must not spell an offending path literally, or the guard it
# is testing would flag this test itself. The fixture body is assembled at runtime.

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$validator = Join-Path $repo 'scripts\validate-powershell.ps1'
$shellPath = (Get-Command $Shell -ErrorAction Stop).Source
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('hls-v7-script-hygiene-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null

function Write-Fixture([string]$Path, [string]$Body) {
    [IO.File]::WriteAllText($Path, $Body, [Text.UTF8Encoding]::new($false))
}

function Invoke-Validator([string]$Target) {
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        # The offending fixture is expected to make the child shell exit non-zero.
        # Windows PowerShell surfaces child stderr as ErrorRecord objects, so keep
        # those records non-terminating here and assert the exit code below.
        $ErrorActionPreference = 'Continue'
        $output = @(& $shellPath -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $validator -Path $Target 2>&1 |
            ForEach-Object { $_.ToString() })
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    return [pscustomobject]@{ ExitCode = $exitCode; Output = ($output -join "`n") }
}

try {
    $clean = Join-Path $tempRoot 'clean.ps1'
    Write-Fixture $clean "Write-Output 'a release script that stays machine independent'`r`n"
    $cleanResult = Invoke-Validator $clean
    if ($cleanResult.ExitCode -ne 0) {
        throw "A script without a user-profile path must pass under ${Shell}. exit=$($cleanResult.ExitCode) output=$($cleanResult.Output)"
    }
    $cleanFlat = (($cleanResult.Output -join "`n") -replace '\s+', ' ')
    if ($cleanFlat -notmatch 'PowerShell syntax validation passed for 1 script') {
        throw "Clean fixture under ${Shell} did not report the expected validation line: $($cleanResult.Output)"
    }

    # Two shapes an offending script can take. The first is the real defect that
    # shipped: an interpreter fallback under a user profile. The second is the
    # machine-independent form that must keep passing, so the guard is not simply
    # "reject anything mentioning a home directory".
    #
    # The offending path is assembled at runtime from a drive letter and a relative
    # tail so this file never spells a rooted profile path itself; otherwise the
    # guard under test would flag its own regression test.
    $profileTail = 'Users\someone\.conda\envs\test\python.exe'
    $offendingBody = "`$python = '" + ('{0}:' -f 'C') + [IO.Path]::DirectorySeparatorChar + $profileTail + "'`r`n"
    foreach ($offender in @(
            @{ Name = 'interpreter-fallback.ps1'; Body = $offendingBody },
            @{ Name = 'profile-via-env.ps1'; Body = "`$out = Join-Path `$env:USERPROFILE 'HLSDownloader\logs'`r`n" }
        )) {
        $path = Join-Path $tempRoot $offender.Name
        # The second body deliberately contains no rooted profile path: the guard
        # looks for one, so this fixture must pass. Using $env:USERPROFILE is the
        # machine-independent form release scripts should keep using.
        if ($offender.Name -eq 'profile-via-env.ps1') {
            Write-Fixture $path $offender.Body
            $result = Invoke-Validator $path
            if ($result.ExitCode -ne 0) {
                throw "A script using `$env:USERPROFILE instead of a literal profile path must pass under ${Shell}. output=$($result.Output)"
            }
            continue
        }
        Write-Fixture $path $offender.Body
        $result = Invoke-Validator $path
        if ($result.ExitCode -eq 0) {
            throw "$($offender.Name) must fail under ${Shell}: a developer-specific profile path is a portability defect. output=$($result.Output)"
        }
        # Windows PowerShell re-renders multi-line error records and wraps long
        # messages at the console width, sometimes mid-word ("pat" + CRLF + "h."),
        # so compare with every whitespace character removed from both sides.
        $flat = (($result.Output -join '') -replace '\s+', '')
        $needle = ('developer-specific Windows user profile path' -replace '\s+', '')
        if ($flat -notmatch $needle) {
            throw "$($offender.Name) failure under ${Shell} did not expose the fail-closed reason: $($result.Output)"
        }
    }

    Write-Output ([ordered]@{
        schema = 1
        passed = $true
        shell = $Shell
        clean_exit = $cleanResult.ExitCode
    } | ConvertTo-Json -Compress)
} finally {
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
