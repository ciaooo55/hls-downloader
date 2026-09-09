# Load MSVC into this PowerShell process. No-op on CI machines that already have cl/link.
$local = "E:\VS\vcvars.ps1"
if (Test-Path -LiteralPath $local) {
    . $local
    return
}
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path -LiteralPath $vswhere) {
    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($vs) {
        $vcvars = Join-Path $vs "VC\Auxiliary\Build\vcvars64.bat"
        if (Test-Path -LiteralPath $vcvars) {
            cmd /c "call `"$vcvars`" >nul && set" | ForEach-Object {
                if ($_ -match "^(.*?)=(.*)$") {
                    [System.Environment]::SetEnvironmentVariable($matches[1], $matches[2], "Process")
                }
            }
        }
    }
}
