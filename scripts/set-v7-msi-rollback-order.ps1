[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$MsiPath,
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\{[0-9A-Fa-f-]{36}\}$')]
    [string]$ProductCode
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'V7HashFunctions.ps1')
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
    # Type 1042 is an installed-file EXE deferred in the execution script.
    # It only creates manifests under INSTALLDIR; MSI owns the HKCU entries.
    Invoke-MsiNonQuery "INSERT INTO ``CustomAction`` (``Action``,``Type``,``Source``,``Target``) VALUES ('$Action',1042,'$SourceFile','$escapedArguments')"
    Invoke-MsiNonQuery "INSERT INTO ``InstallExecuteSequence`` (``Action``,``Condition``,``Sequence``) VALUES ('$Action','$escapedCondition',$Sequence)"
}

function Set-MsiRegistryDefaultValue(
    [string]$Id,
    [string]$Key,
    [string]$Value,
    [string]$Component
) {
    $escapedKey = $Key.Replace("'", "''")
    $escapedValue = $Value.Replace("'", "''")
    Invoke-MsiNonQuery "DELETE FROM ``Registry`` WHERE ``Registry``='$Id'"
    Invoke-MsiNonQuery "INSERT INTO ``Registry`` (``Registry``,``Root``,``Key``,``Name``,``Value``,``Component_``) VALUES ('$Id',1,'$escapedKey',NULL,'$escapedValue','$Component')"
}

try {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    # 1 is the transacted database mode. Commit happens only after all gates pass.
    $database = $installer.GetType().InvokeMember(
        'OpenDatabase', 'InvokeMethod', $null, $installer, @($resolved, 1)
    )
    $current = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='RemoveExistingProducts'"
    $originalProductCode = Invoke-MsiStringQuery "SELECT ``Value`` FROM ``Property`` WHERE ``Property``='ProductCode'"
    $upgradableAttributes = Invoke-MsiScalarQuery "SELECT ``Attributes`` FROM ``Upgrade`` WHERE ``ActionProperty``='JP_UPGRADABLE_FOUND'"
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
    $engineComponent = Invoke-MsiStringQuery "SELECT ``Component_`` FROM ``File`` WHERE ``File``='$engineFile'"
    if ([String]::IsNullOrWhiteSpace($engineComponent)) {
        throw 'MSI does not expose the HLSDownloaderEngine.exe component for Native Host registry ownership.'
    }
    Invoke-MsiNonQuery "UPDATE ``Property`` SET ``Value``='$ProductCode' WHERE ``Property``='ProductCode'"
    if ($null -eq $upgradableAttributes) {
        throw 'MSI is missing the JP_UPGRADABLE_FOUND upgrade rule.'
    }
    # Candidate rebuilds keep the product version but receive a new ProductCode.
    # Include VersionMax so a newer candidate replaces the earlier one cleanly.
    $sameVersionAttributes = [int]$upgradableAttributes -bor 0x200
    $view = $database.GetType().InvokeMember(
        'OpenView', 'InvokeMethod', $null, $database,
        @("SELECT * FROM ``Upgrade`` WHERE ``ActionProperty``='JP_UPGRADABLE_FOUND'")
    )
    $upgradeRecord = $null
    try {
        $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
        $upgradeRecord = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
        if ($null -eq $upgradeRecord) {
            throw 'MSI is missing the JP_UPGRADABLE_FOUND upgrade row.'
        }
        $view.GetType().InvokeMember('Modify', 'InvokeMethod', $null, $view, @([int]6, $upgradeRecord)) | Out-Null
        $upgradeRecord.GetType().InvokeMember('IntegerData', 'SetProperty', $null, $upgradeRecord, @(5, $sameVersionAttributes)) | Out-Null
        $view.GetType().InvokeMember('Modify', 'InvokeMethod', $null, $view, @([int]1, $upgradeRecord)) | Out-Null
    } finally {
        if ($null -ne $upgradeRecord) {
            [Runtime.InteropServices.Marshal]::FinalReleaseComObject($upgradeRecord) | Out-Null
        }
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) | Out-Null
        $view = $null
    }

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

    $prepareSequence = [int]$installFiles + 10
    if ($prepareSequence -ge [int]$finalize) {
        throw 'MSI does not provide legal Native Host registration action slots.'
    }
    foreach ($legacyAction in @('V7RegisterNativeHost', 'V7UnregisterNativeHost')) {
        Invoke-MsiNonQuery "DELETE FROM ``InstallExecuteSequence`` WHERE ``Action``='$legacyAction'"
        Invoke-MsiNonQuery "DELETE FROM ``CustomAction`` WHERE ``Action``='$legacyAction'"
    }
    Set-MsiExecutableAction 'V7PrepareNativeHostManifests' $engineFile '--prepare-native-host-manifests' 'NOT REMOVE~="ALL"' $prepareSequence
    $nativeHostRegistry = @(
        @('V7NativeHostChrome', 'Software\Google\Chrome\NativeMessagingHosts\com.ciaooo55.hls_downloader', '[INSTALLDIR]app\resources\HLSDownloaderNativeHost.chrome.json'),
        @('V7NativeHostEdge', 'Software\Microsoft\Edge\NativeMessagingHosts\com.ciaooo55.hls_downloader', '[INSTALLDIR]app\resources\HLSDownloaderNativeHost.chrome.json'),
        @('V7NativeHostBrave', 'Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.ciaooo55.hls_downloader', '[INSTALLDIR]app\resources\HLSDownloaderNativeHost.chrome.json'),
        @('V7NativeHostChromium', 'Software\Chromium\NativeMessagingHosts\com.ciaooo55.hls_downloader', '[INSTALLDIR]app\resources\HLSDownloaderNativeHost.chrome.json'),
        @('V7NativeHostVivaldi', 'Software\Vivaldi\NativeMessagingHosts\com.ciaooo55.hls_downloader', '[INSTALLDIR]app\resources\HLSDownloaderNativeHost.chrome.json'),
        @('V7NativeHostOpera', 'Software\Opera Software\NativeMessagingHosts\com.ciaooo55.hls_downloader', '[INSTALLDIR]app\resources\HLSDownloaderNativeHost.chrome.json'),
        @('V7NativeHostFirefox', 'Software\Mozilla\NativeMessagingHosts\com.ciaooo55.hls_downloader', '[INSTALLDIR]app\resources\HLSDownloaderNativeHost.firefox.json')
    )
    foreach ($entry in $nativeHostRegistry) {
        Set-MsiRegistryDefaultValue $entry[0] $entry[1] $entry[2] $engineComponent
    }

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
    $verifiedUpgradableAttributes = Invoke-MsiScalarQuery "SELECT ``Attributes`` FROM ``Upgrade`` WHERE ``ActionProperty``='JP_UPGRADABLE_FOUND'"
    $verifiedPrepareSequence = Invoke-MsiScalarQuery "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='V7PrepareNativeHostManifests'"
    $verifiedPrepareType = Invoke-MsiScalarQuery "SELECT ``Type`` FROM ``CustomAction`` WHERE ``Action``='V7PrepareNativeHostManifests'"
    $verifiedPrepareTarget = Invoke-MsiStringQuery "SELECT ``Target`` FROM ``CustomAction`` WHERE ``Action``='V7PrepareNativeHostManifests'"
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
    if (([int]$verifiedUpgradableAttributes -band 0x200) -eq 0) {
        throw 'MSI same-version upgrade verification failed.'
    }
    if (
        [int]$verifiedPrepareSequence -ne $prepareSequence -or
        [int]$verifiedPrepareType -ne 1042 -or
        $verifiedPrepareTarget -ne '--prepare-native-host-manifests'
    ) {
        throw 'MSI Native Host registration action verification failed.'
    }
    foreach ($entry in $nativeHostRegistry) {
        $id = $entry[0]
        $verifiedRoot = Invoke-MsiScalarQuery "SELECT ``Root`` FROM ``Registry`` WHERE ``Registry``='$id'"
        $verifiedKey = Invoke-MsiStringQuery "SELECT ``Key`` FROM ``Registry`` WHERE ``Registry``='$id'"
        $verifiedValue = Invoke-MsiStringQuery "SELECT ``Value`` FROM ``Registry`` WHERE ``Registry``='$id'"
        $verifiedComponent = Invoke-MsiStringQuery "SELECT ``Component_`` FROM ``Registry`` WHERE ``Registry``='$id'"
        if (
            [int]$verifiedRoot -ne 1 -or
            $verifiedKey -ne $entry[1] -or
            $verifiedValue -ne $entry[2] -or
            $verifiedComponent -ne $engineComponent
        ) {
            throw "MSI Native Host registry verification failed for $id."
        }
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
    native_host_prepare_type = [int]$verifiedPrepareType
    native_host_prepare_sequence = [int]$verifiedPrepareSequence
    native_host_registry_count = $nativeHostRegistry.Count
    same_version_upgrade_attributes = [int]$verifiedUpgradableAttributes
    sha256_before = $beforeHash
    sha256_after = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash
} | ConvertTo-Json
