[CmdletBinding()]
param(
    [string]$CandidateDir = 'artifacts\v7-productization\candidate',
    [int]$MaxArtifactMiB = 384
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath $CandidateDir).Path
$limitBytes = [int64]$MaxArtifactMiB * 1MB
$files = @(Get-ChildItem -LiteralPath $root -File | Sort-Object Name)
if ($files.Count -eq 0) { throw "No candidate files found under $root" }

$topLevel = @($files | ForEach-Object {
    [ordered]@{
        name = $_.Name
        bytes = [int64]$_.Length
        mib = [Math]::Round($_.Length / 1MB, 2)
    }
})

$portable = $files | Where-Object { $_.Name -match 'Portable.*\.zip$' } | Select-Object -First 1
$portableEntries = @()
$groups = @{}
if ($portable) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($portable.FullName)
    try {
        foreach ($entry in $zip.Entries) {
            if ([String]::IsNullOrWhiteSpace($entry.Name)) { continue }
            $parts = $entry.FullName -split '[\\/]'
            $group = if ($parts.Count -ge 4 -and $parts[1] -eq 'app' -and $parts[2] -eq 'resources') {
                "app/resources/$($parts[3])"
            } elseif ($parts.Count -ge 3) {
                "$($parts[1])/$($parts[2])"
            } elseif ($parts.Count -ge 2) {
                $parts[1]
            } else {
                $parts[0]
            }
            if (-not $groups.ContainsKey($group)) {
                $groups[$group] = [ordered]@{ bytes = [int64]0; compressed_bytes = [int64]0; files = 0 }
            }
            $groups[$group].bytes += [int64]$entry.Length
            $groups[$group].compressed_bytes += [int64]$entry.CompressedLength
            $groups[$group].files += 1
            $portableEntries += [pscustomobject]@{
                path = $entry.FullName
                bytes = [int64]$entry.Length
                compressed_bytes = [int64]$entry.CompressedLength
            }
        }
    } finally {
        $zip.Dispose()
    }
}

$largestEntries = @($portableEntries | Sort-Object bytes -Descending | Select-Object -First 40 | ForEach-Object {
    [ordered]@{
        path = $_.path
        bytes = $_.bytes
        mib = [Math]::Round($_.bytes / 1MB, 2)
        compressed_bytes = $_.compressed_bytes
        compressed_mib = [Math]::Round($_.compressed_bytes / 1MB, 2)
    }
})

$groupReport = @($groups.GetEnumerator() | ForEach-Object {
    [pscustomobject]@{
        name = $_.Key
        bytes = [int64]$_.Value.bytes
        compressed_bytes = [int64]$_.Value.compressed_bytes
        files = [int]$_.Value.files
    }
} | Sort-Object bytes -Descending | ForEach-Object {
    [ordered]@{
        name = $_.name
        bytes = $_.bytes
        mib = [Math]::Round($_.bytes / 1MB, 2)
        compressed_bytes = $_.compressed_bytes
        compressed_mib = [Math]::Round($_.compressed_bytes / 1MB, 2)
        files = $_.files
    }
})

$report = [ordered]@{
    schema = 1
    generated_at_utc = [DateTime]::UtcNow.ToString('o')
    max_artifact_mib = $MaxArtifactMiB
    top_level_artifacts = $topLevel
    portable_groups = $groupReport
    portable_largest_entries = $largestEntries
}

$out = Join-Path $root 'PACKAGE-SIZE-REPORT.json'
[IO.File]::WriteAllText($out, ($report | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))

$oversized = @($files | Where-Object { $_.Extension -in @('.exe', '.msi', '.zip') -and $_.Length -gt $limitBytes })
Write-Host "Candidate package size report: $out"
$topLevel | Format-Table -AutoSize | Out-Host
if ($groupReport.Count -gt 0) {
    Write-Host 'Largest portable groups:'
    $groupReport | Select-Object -First 15 | Format-Table -AutoSize | Out-Host
}
if ($oversized.Count -gt 0) {
    throw "Candidate artifact size regression: limit is $MaxArtifactMiB MiB; oversized: $($oversized.Name -join ', ')"
}
