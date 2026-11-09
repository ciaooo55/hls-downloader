[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
$cargo = if ($cargoCommand) { $cargoCommand.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
$forbidden = @('hls-native-shell', 'rusqlite', 'libsqlite3-sys', 'russh', 'russh-sftp')
foreach ($package in @('hls-protocol', 'hls-client', 'hls-native-clients', 'hls-native-ui')) {
    $tree = & $cargo tree --locked --manifest-path (Join-Path $repo 'Cargo.toml') -p $package --edges normal --prefix none --format '{p}'
    if ($LASTEXITCODE -ne 0) { throw "Cannot inspect dependency graph for $package" }
    foreach ($line in $tree) {
        $name = ([string]$line -split '\s+')[0]
        if ($forbidden -contains $name) { throw "$package must not depend on $name" }
    }
}
Write-Output 'Native client dependency boundaries passed.'
