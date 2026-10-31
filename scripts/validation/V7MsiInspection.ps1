# Read-only Windows Installer inspection shared by lifecycle gates.
function Get-V7MsiProperty([string]$Path, [string]$Property) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    $database = $null
    $view = $null
    $record = $null
    try {
        $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($Path, 0))
        $sql = "SELECT ``Value`` FROM ``Property`` WHERE ``Property``='$($Property.Replace("'", "''"))'"
        $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @($sql))
        $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
        $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
        if ($null -eq $record) { return $null }
        return [string]$record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, 1)
    } finally {
        foreach ($item in @($record, $view, $database, $installer)) {
            if ($null -ne $item) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($item) | Out-Null }
        }
    }
}

function Get-V7InstalledProduct([string]$UpgradeCode) {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    try {
        foreach ($productCode in @($installer.RelatedProducts($UpgradeCode))) {
            return [ordered]@{
                product_code = [string]$productCode
                version = [string]$installer.ProductInfo($productCode, 'VersionString')
            }
        }
        return $null
    } finally {
        [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null
    }
}
