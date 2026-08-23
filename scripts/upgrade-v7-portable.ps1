[CmdletBinding(DefaultParameterSetName='Upgrade')]
param(
    [Parameter(Mandatory=$true, ParameterSetName='Upgrade')][string]$SourceDir,
    [Parameter(Mandatory=$true, ParameterSetName='Upgrade')][string]$TargetDir,
    [Parameter(Mandatory=$true, ParameterSetName='Rollback')][switch]$Rollback,
    [Parameter(Mandatory=$true, ParameterSetName='Rollback')][string]$RollbackDir
)

$ErrorActionPreference = 'Stop'
$preserved = @('config.json','data.db','data.db-shm','data.db-wal','downloads','.tasks')

function Full([string]$Path) { [IO.Path]::GetFullPath($Path).TrimEnd('\','/') }
function Stop-V7Processes([string]$Root) {
    $prefix = (Full $Root) + [IO.Path]::DirectorySeparatorChar
    Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
        Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase) } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}
function Assert-AppImage([string]$Root) {
    foreach ($name in @('HLSDownloader.exe','app','runtime')) {
        if (-not (Test-Path -LiteralPath (Join-Path $Root $name))) { throw "v7 portable image is missing ${name}: $Root" }
    }
}
function Copy-Preserved([string]$From, [string]$To) {
    foreach ($name in $preserved) {
        $source = Join-Path $From $name
        if (-not (Test-Path -LiteralPath $source)) { continue }
        $destination = Join-Path $To $name
        if (Test-Path -LiteralPath $destination) { Remove-Item -LiteralPath $destination -Recurse -Force }
        Move-Item -LiteralPath $source -Destination $destination
    }
}

if ($PSCmdlet.ParameterSetName -eq 'Rollback') {
    $target = Full $RollbackDir
    $backup = "$target.v7-backup"
    $current = "$target.v7-rollback-current"
    if (-not (Test-Path -LiteralPath $backup -PathType Container)) { throw "v7 rollback backup is missing: $backup" }
    Stop-V7Processes $target
    if (Test-Path -LiteralPath $current) { Remove-Item -LiteralPath $current -Recurse -Force }
    Move-Item -LiteralPath $target -Destination $current
    Move-Item -LiteralPath $backup -Destination $target
    Copy-Preserved $current $target
    Remove-Item -LiteralPath $current -Recurse -Force
    Write-Host "v7 portable rollback completed: $target"
    exit 0
}

$source = Full $SourceDir
$target = Full $TargetDir
Assert-AppImage $source
Assert-AppImage $target
if ([String]::Equals($source, $target, [StringComparison]::OrdinalIgnoreCase)) { throw 'v7 source and target must be different directories' }
$backup = "$target.v7-backup"
if (Test-Path -LiteralPath $backup) { throw "v7 upgrade backup already exists; finalize or rollback first: $backup" }
Stop-V7Processes $target
Move-Item -LiteralPath $target -Destination $backup
try {
    Move-Item -LiteralPath $source -Destination $target
    Copy-Preserved $backup $target
    Write-Host "v7 portable upgrade completed: $target"
} catch {
    if (Test-Path -LiteralPath $target) { Move-Item -LiteralPath $target -Destination $source -Force -ErrorAction SilentlyContinue }
    if (Test-Path -LiteralPath $backup) { Move-Item -LiteralPath $backup -Destination $target -Force -ErrorAction SilentlyContinue }
    throw
}
