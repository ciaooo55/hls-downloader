[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$MsiPath
)

$ErrorActionPreference = 'Stop'
$resolved = (Resolve-Path -LiteralPath $MsiPath).Path
$beforeHash = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash
$installer = $null
$database = $null
$view = $null

function Invoke-MsiScalarQuery([string]$Sql) {
    $localView = $database.GetType().InvokeMember(
        'OpenView', 'InvokeMethod', $null, $database, @($Sql)
    )
    try {
        $localView.GetType().InvokeMember(
            'Execute', 'InvokeMethod', $null, $localView, $null
        ) | Out-Null
        $record = $localView.GetType().InvokeMember(
            'Fetch', 'InvokeMethod', $null, $localView, $null
        )
        if ($null -eq $record) { return $null }
        try {
            return $record.GetType().InvokeMember(
                'IntegerData', 'GetProperty', $null, $record, 1
            )
        } finally {
            [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) | Out-Null
        }
    } finally {
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($localView) | Out-Null
    }
}

try {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    # 1 is the transacted database mode. Commit happens only after all gates pass.
    $database = $installer.GetType().InvokeMember(
        'OpenDatabase', 'InvokeMethod', $null, $installer, @($resolved, 1)
    )
    $current = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='RemoveExistingProducts'"
    $installFiles = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='InstallFiles'"
    $finalize = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='InstallFinalize'"
    if ($null -eq $current -or $null -eq $installFiles -or $null -eq $finalize) {
        throw 'MSI is missing the upgrade transaction actions required for rollback.'
    }
    # jpackage does not emit InstallExecute. The rollback-safe legal slot for
    # RemoveExistingProducts is therefore immediately after InstallFinalize:
    # a failed new-product transaction never removes the installed version.
    $target = [int]$finalize + 10
    if ($target -le [int]$finalize) {
        throw 'MSI does not provide a rollback-safe sequence slot after InstallFinalize.'
    }
    if ([int]$current -ne $target) {
        $sql = "UPDATE ``InstallExecuteSequence`` SET ``Sequence``=$target WHERE ``Action``='RemoveExistingProducts'"
        $view = $database.GetType().InvokeMember(
            'OpenView', 'InvokeMethod', $null, $database, @($sql)
        )
        $view.GetType().InvokeMember(
            'Execute', 'InvokeMethod', $null, $view, $null
        ) | Out-Null
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) | Out-Null
        $view = $null
        $database.GetType().InvokeMember(
            'Commit', 'InvokeMethod', $null, $database, $null
        ) | Out-Null
    }
} finally {
    if ($null -ne $view) {
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) | Out-Null
    }
    if ($null -ne $database) {
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) | Out-Null
    }
    if ($null -ne $installer) {
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null
    }
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}

$verifyInstaller = New-Object -ComObject WindowsInstaller.Installer
$verifyDatabase = $null
try {
    $verifyDatabase = $verifyInstaller.GetType().InvokeMember(
        'OpenDatabase', 'InvokeMethod', $null, $verifyInstaller, @($resolved, 0)
    )
    $database = $verifyDatabase
    $verified = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='RemoveExistingProducts'"
    if ([int]$verified -ne $target) {
        throw "MSI rollback sequence verification failed: expected $target, got $verified."
    }
} finally {
    $database = $null
    if ($null -ne $verifyDatabase) {
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($verifyDatabase) | Out-Null
    }
    [Runtime.InteropServices.Marshal]::FinalReleaseComObject($verifyInstaller) | Out-Null
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}

[ordered]@{
    msi = $resolved
    original_sequence = [int]$current
    verified_sequence = [int]$verified
    install_files_sequence = [int]$installFiles
    install_finalize_sequence = [int]$finalize
    sha256_before = $beforeHash
    sha256_after = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash
} | ConvertTo-Json
