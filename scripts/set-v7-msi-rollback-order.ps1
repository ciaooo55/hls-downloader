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

function Invoke-MsiStringQuery([string]$Sql) {
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
                'StringData', 'GetProperty', $null, $record, 1
            )
        } finally {
            [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) | Out-Null
        }
    } finally {
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($localView) | Out-Null
    }
}

function Invoke-MsiNonQuery([string]$Sql) {
    $localView = $database.GetType().InvokeMember(
        'OpenView', 'InvokeMethod', $null, $database, @($Sql)
    )
    try {
        $localView.GetType().InvokeMember(
            'Execute', 'InvokeMethod', $null, $localView, $null
        ) | Out-Null
    } finally {
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($localView) | Out-Null
    }
}

function Find-MsiFileKey([string]$LongName) {
    $localView = $database.GetType().InvokeMember(
        'OpenView', 'InvokeMethod', $null, $database, @('SELECT `File`,`FileName` FROM `File`')
    )
    try {
        $localView.GetType().InvokeMember(
            'Execute', 'InvokeMethod', $null, $localView, $null
        ) | Out-Null
        while ($true) {
            $record = $localView.GetType().InvokeMember(
                'Fetch', 'InvokeMethod', $null, $localView, $null
            )
            if ($null -eq $record) { break }
            try {
                $key = $record.GetType().InvokeMember(
                    'StringData', 'GetProperty', $null, $record, 1
                )
                $fileName = $record.GetType().InvokeMember(
                    'StringData', 'GetProperty', $null, $record, 2
                )
                $candidate = @($fileName -split '\|')[-1]
                if ($candidate -eq $LongName) { return $key }
            } finally {
                [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) | Out-Null
            }
        }
    } finally {
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($localView) | Out-Null
    }
    return $null
}

function Set-MsiExecutableAction(
    [string]$Action,
    [string]$SourceFile,
    [string]$Arguments,
    [string]$Condition,
    [int]$Sequence
) {
    $escapedArguments = $Arguments.Replace("'", "''")
    $escapedCondition = $Condition.Replace("'", "''")
    Invoke-MsiNonQuery "DELETE FROM ``InstallExecuteSequence`` WHERE ``Action``='$Action'"
    Invoke-MsiNonQuery "DELETE FROM ``CustomAction`` WHERE ``Action``='$Action'"
    # Type 18 launches an installed executable. Type 64 keeps registration repair
    # from invalidating an otherwise usable per-user install, so the total is 82.
    Invoke-MsiNonQuery "INSERT INTO ``CustomAction`` (``Action``,``Type``,``Source``,``Target``) VALUES ('$Action',82,'$SourceFile','$escapedArguments')"
    Invoke-MsiNonQuery "INSERT INTO ``InstallExecuteSequence`` (``Action``,``Condition``,``Sequence``) VALUES ('$Action','$escapedCondition',$Sequence)"
}

try {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    # 1 is the transacted database mode. Commit happens only after all gates pass.
    $database = $installer.GetType().InvokeMember(
        'OpenDatabase', 'InvokeMethod', $null, $installer, @($resolved, 1)
    )
    $current = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='RemoveExistingProducts'"
    $installFiles = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='InstallFiles'"
    $removeFiles = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='RemoveFiles'"
    $finalize = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='InstallFinalize'"
    $engineFile = Find-MsiFileKey 'HLSDownloaderEngine.exe'
    if ($null -eq $current -or $null -eq $installFiles -or $null -eq $removeFiles -or $null -eq $finalize) {
        throw 'MSI is missing the install/upgrade actions required for rollback and Native Host registration.'
    }
    if ([String]::IsNullOrWhiteSpace($engineFile)) {
        throw 'MSI does not contain HLSDownloaderEngine.exe for Native Host registration actions.'
    }

    # jpackage does not emit InstallExecute. The rollback-safe legal slot for
    # RemoveExistingProducts is immediately after InstallFinalize.
    $target = [int]$finalize + 10
    if ($target -le [int]$finalize) {
        throw 'MSI does not provide a rollback-safe sequence slot after InstallFinalize.'
    }
    if ([int]$current -ne $target) {
        Invoke-MsiNonQuery "UPDATE ``InstallExecuteSequence`` SET ``Sequence``=$target WHERE ``Action``='RemoveExistingProducts'"
    }

    $registerSequence = [int]$installFiles + 10
    $unregisterSequence = [int]$removeFiles - 10
    if ($registerSequence -ge [int]$finalize -or $unregisterSequence -le 0) {
        throw 'MSI does not provide legal Native Host registration action slots.'
    }
    Set-MsiExecutableAction 'V7RegisterNativeHost' $engineFile '--register-native-host' 'NOT REMOVE~="ALL"' $registerSequence
    # A major upgrade must leave the new product's repaired registration intact.
    Set-MsiExecutableAction 'V7UnregisterNativeHost' $engineFile '--unregister-native-host' 'REMOVE~="ALL" AND NOT UPGRADINGPRODUCTCODE' $unregisterSequence

    $database.GetType().InvokeMember(
        'Commit', 'InvokeMethod', $null, $database, $null
    ) | Out-Null
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
    $verifiedRegisterSequence = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='V7RegisterNativeHost'"
    $verifiedUnregisterSequence = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='V7UnregisterNativeHost'"
    $verifiedRegisterTarget = Invoke-MsiStringQuery "SELECT ``Target`` FROM ``CustomAction`` WHERE ``Action``='V7RegisterNativeHost'"
    $verifiedUnregisterTarget = Invoke-MsiStringQuery "SELECT ``Target`` FROM ``CustomAction`` WHERE ``Action``='V7UnregisterNativeHost'"
    if ([int]$verified -ne $target) {
        throw "MSI rollback sequence verification failed: expected $target, got $verified."
    }
    if (
        [int]$verifiedRegisterSequence -ne $registerSequence -or
        [int]$verifiedUnregisterSequence -ne $unregisterSequence -or
        $verifiedRegisterTarget -ne '--register-native-host' -or
        $verifiedUnregisterTarget -ne '--unregister-native-host'
    ) {
        throw 'MSI Native Host registration action verification failed.'
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
    native_host_engine_file = $engineFile
    native_host_register_sequence = [int]$verifiedRegisterSequence
    native_host_unregister_sequence = [int]$verifiedUnregisterSequence
    sha256_before = $beforeHash
    sha256_after = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash
} | ConvertTo-Json
