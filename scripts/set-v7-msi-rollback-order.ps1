[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$MsiPath,
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\{[0-9A-Fa-f-]{36}\}$')]
    [string]$ProductCode
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
    # Type 1042 is an installed-file EXE (18) deferred in the execution script
    # (1024). It remains impersonated so Native Host registration writes HKCU.
    Invoke-MsiNonQuery "INSERT INTO ``CustomAction`` (``Action``,``Type``,``Source``,``Target``) VALUES ('$Action',1042,'$SourceFile','$escapedArguments')"
    Invoke-MsiNonQuery "INSERT INTO ``InstallExecuteSequence`` (``Action``,``Condition``,``Sequence``) VALUES ('$Action','$escapedCondition',$Sequence)"
}

try {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    # 1 is the transacted database mode. Commit happens only after all gates pass.
    $database = $installer.GetType().InvokeMember(
        'OpenDatabase', 'InvokeMethod', $null, $installer, @($resolved, 1)
    )
    $current = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='RemoveExistingProducts'"
    $originalProductCode = Invoke-MsiStringQuery "SELECT ``Value`` FROM ``Property`` WHERE ``Property``='ProductCode'"
    $initialize = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='InstallInitialize'"
    $installFiles = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='InstallFiles'"
    $removeFiles = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='RemoveFiles'"
    $finalize = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='InstallFinalize'"
    $engineFile = Find-MsiFileKey 'HLSDownloaderEngine.exe'
    if ($null -eq $current -or $null -eq $initialize -or $null -eq $installFiles -or $null -eq $removeFiles -or $null -eq $finalize) {
        throw 'MSI is missing the install/upgrade actions required for rollback and Native Host registration.'
    }
    if ([String]::IsNullOrWhiteSpace($engineFile)) {
        throw 'MSI does not contain HLSDownloaderEngine.exe for Native Host registration actions.'
    }
    Invoke-MsiNonQuery "UPDATE ``Property`` SET ``Value``='$ProductCode' WHERE ``Property``='ProductCode'"

    # jpackage restores the saved directory for RemoveFoldersEx only. Standard
    # ARP uninstall also needs it for installed-file custom actions.
    $installDirValue = Invoke-MsiStringQuery "SELECT ``Name`` FROM ``Registry`` WHERE ``Value``='[INSTALLDIR]'"
    if ([String]::IsNullOrWhiteSpace($installDirValue)) {
        throw 'MSI does not persist INSTALLDIR for maintenance operations.'
    }
    $installDirSearch = Invoke-MsiStringQuery "SELECT ``Signature_`` FROM ``RegLocator`` WHERE ``Name``='$($installDirValue.Replace("'", "''"))'"
    if ([String]::IsNullOrWhiteSpace($installDirSearch)) {
        throw 'MSI is missing the persisted install-directory registry search.'
    }
    Invoke-MsiNonQuery "DELETE FROM ``AppSearch`` WHERE ``Property``='INSTALLDIR'"
    Invoke-MsiNonQuery "INSERT INTO ``AppSearch`` (``Property``,``Signature_``) VALUES ('INSTALLDIR','$installDirSearch')"

    # Remove the old jpackage product before installing new files; its uninstall
    # recursively removes INSTALLDIR even when component GUIDs are stable.
    $target = [int]$initialize + 10
    if ($target -le [int]$initialize -or $target -ge [int]$installFiles) {
        throw 'MSI does not provide a major-upgrade slot before InstallFiles.'
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
    $verifiedProductCode = Invoke-MsiStringQuery "SELECT ``Value`` FROM ``Property`` WHERE ``Property``='ProductCode'"
    $verifiedRegisterSequence = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='V7RegisterNativeHost'"
    $verifiedUnregisterSequence = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='V7UnregisterNativeHost'"
    $verifiedRegisterType = Invoke-MsiScalarQuery "SELECT ``Type`` FROM ``CustomAction`` WHERE ``Action``='V7RegisterNativeHost'"
    $verifiedUnregisterType = Invoke-MsiScalarQuery "SELECT ``Type`` FROM ``CustomAction`` WHERE ``Action``='V7UnregisterNativeHost'"
    $verifiedRegisterTarget = Invoke-MsiStringQuery "SELECT ``Target`` FROM ``CustomAction`` WHERE ``Action``='V7RegisterNativeHost'"
    $verifiedUnregisterTarget = Invoke-MsiStringQuery "SELECT ``Target`` FROM ``CustomAction`` WHERE ``Action``='V7UnregisterNativeHost'"
    $verifiedInstallDirSearch = Invoke-MsiStringQuery "SELECT ``Signature_`` FROM ``AppSearch`` WHERE ``Property``='INSTALLDIR'"
    if ($verifiedInstallDirSearch -ne $installDirSearch) {
        throw 'MSI maintenance install-directory search verification failed.'
    }
    if ([int]$verified -ne $target) {
        throw "MSI rollback sequence verification failed: expected $target, got $verified."
    }
    if ($verifiedProductCode -ne $ProductCode) {
        throw "MSI ProductCode verification failed: expected $ProductCode, got $verifiedProductCode."
    }
    if (
        [int]$verifiedRegisterSequence -ne $registerSequence -or
        [int]$verifiedUnregisterSequence -ne $unregisterSequence -or
        [int]$verifiedRegisterType -ne 1042 -or
        [int]$verifiedUnregisterType -ne 1042 -or
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
    original_product_code = $originalProductCode
    verified_product_code = $verifiedProductCode
    original_sequence = [int]$current
    verified_sequence = [int]$verified
    install_initialize_sequence = [int]$initialize
    install_files_sequence = [int]$installFiles
    install_finalize_sequence = [int]$finalize
    native_host_engine_file = $engineFile
    native_host_register_type = [int]$verifiedRegisterType
    native_host_unregister_type = [int]$verifiedUnregisterType
    native_host_register_sequence = [int]$verifiedRegisterSequence
    native_host_unregister_sequence = [int]$verifiedUnregisterSequence
    sha256_before = $beforeHash
    sha256_after = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash
} | ConvertTo-Json
