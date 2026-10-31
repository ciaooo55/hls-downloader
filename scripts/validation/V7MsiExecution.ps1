function Invoke-V7Msi([string[]]$Arguments, [string]$LogPath) {
    $allArguments = @($Arguments) + @('/norestart', 'REBOOT=ReallySuppress', '/l*v', $LogPath)
    $quotedArguments = @($allArguments | ForEach-Object {
        # Quote MSI property values; quoting the entire KEY=value changes its parsing.
        if ($_ -match '^([A-Za-z_][A-Za-z0-9_]*)=(.*)$') {
            $Matches[1] + '="' + $Matches[2].Replace('"', '""') + '"'
        } elseif ($_ -match '[\s"]') {
            '"' + $_.Replace('"', '""') + '"'
        } else { $_ }
    })
    $process = Start-Process -FilePath 'msiexec.exe' -ArgumentList $quotedArguments -Wait -PassThru
    return [int]$process.ExitCode
}

