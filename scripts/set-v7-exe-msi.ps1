[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ExePath,
    [Parameter(Mandatory = $true)][string]$MsiPath
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'V7HashFunctions.ps1')
$exe = (Resolve-Path -LiteralPath $ExePath).Path
$msi = (Resolve-Path -LiteralPath $MsiPath).Path
$installer = New-Object -ComObject WindowsInstaller.Installer
$database = $null
$view = $null
$record = $null
try {
    $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($msi, 0))
    $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @('SELECT `Value` FROM `Property` WHERE `Property`=''ProductCode'''))
    $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
    $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
    if ($null -eq $record) { throw 'MSI has no ProductCode.' }
    $productCode = $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, 1)
    if ($productCode -notmatch '^\{[0-9a-fA-F-]{36}\}$') { throw 'MSI has an invalid ProductCode.' }
} finally {
    foreach ($obj in @($record, $view, $database, $installer)) {
        if ($null -ne $obj) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($obj) | Out-Null }
    }
}

# jpackage builds a separate MSI for EXE. Sync installation and uninstall IDs.
# Update before signing, retaining the wrapper, icon and version resources.
if (-not ('V7InstallerResources' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;

public static class V7InstallerResources {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr FindResourceW(IntPtr module, string name, IntPtr type);
    [DllImport("kernel32.dll", EntryPoint = "FindResourceW", SetLastError = true)]
    private static extern IntPtr FindManifest(IntPtr module, IntPtr name, IntPtr type);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint SizeofResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr LockResource(IntPtr resource);
    [DllImport("kernel32.dll")]
    private static extern bool FreeLibrary(IntPtr module);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr BeginUpdateResourceW(string path, bool deleteExisting);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool UpdateResourceW(IntPtr handle, IntPtr type, string name, ushort language, byte[] data, uint size);
    [DllImport("kernel32.dll", EntryPoint = "UpdateResourceW", SetLastError = true)]
    private static extern bool UpdateManifest(IntPtr handle, IntPtr type, IntPtr name, ushort language, byte[] data, uint size);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool EndUpdateResourceW(IntPtr handle, bool discard);

    private static byte[] Read(string path, string name, bool manifest = false) {
        // LOAD_LIBRARY_AS_DATAFILE prevents executing installer code.
        IntPtr module = LoadLibraryExW(path, IntPtr.Zero, 2);
        if (module == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            IntPtr resource = manifest ? FindManifest(module, new IntPtr(1), new IntPtr(24)) : FindResourceW(module, name, new IntPtr(10));
            if (resource == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            uint size = SizeofResource(module, resource);
            IntPtr data = LockResource(LoadResource(module, resource));
            if (size == 0 || data == IntPtr.Zero) throw new InvalidDataException("Empty installer resource: " + name);
            byte[] bytes = new byte[checked((int)size)];
            Marshal.Copy(data, bytes, 0, bytes.Length);
            return bytes;
        } finally { FreeLibrary(module); }
    }

    private static string Hash(byte[] bytes) {
        using (var hash = SHA256.Create()) {
            return BitConverter.ToString(hash.ComputeHash(bytes)).Replace("-", "").ToLowerInvariant();
        }
    }

    public static string Replace(string exe, string msi, string productCode) {
        // Require the existing jpackage wrapper resources.
        Read(exe, "msi");
        Read(exe, "product_code");
        byte[] payload = File.ReadAllBytes(msi);
        byte[] code = Encoding.UTF8.GetBytes(productCode);
        string manifestText = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><assembly xmlns=\"urn:schemas-microsoft-com:asm.v1\" manifestVersion=\"1.0\"><assemblyIdentity type=\"win32\" name=\"HLSDownloader.Installer\" version=\"1.0.0.0\"/><trustInfo xmlns=\"urn:schemas-microsoft-com:asm.v3\"><security><requestedPrivileges><requestedExecutionLevel level=\"requireAdministrator\" uiAccess=\"false\"/></requestedPrivileges></security></trustInfo></assembly>";
        byte[] manifestBytes = Encoding.UTF8.GetBytes(manifestText);
        IntPtr handle = BeginUpdateResourceW(exe, false);
        if (handle == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            if (!UpdateResourceW(handle, new IntPtr(10), "msi", 0, payload, (uint)payload.Length) ||
                !UpdateResourceW(handle, new IntPtr(10), "product_code", 0, code, (uint)code.Length) ||
                !UpdateManifest(handle, new IntPtr(24), new IntPtr(1), 0, manifestBytes, (uint)manifestBytes.Length)) {
                throw new Win32Exception(Marshal.GetLastWin32Error());
            }
            IntPtr pending = handle;
            handle = IntPtr.Zero;
            if (!EndUpdateResourceW(pending, false)) throw new Win32Exception(Marshal.GetLastWin32Error());
        } finally {
            if (handle != IntPtr.Zero) EndUpdateResourceW(handle, true);
        }
        string expected = Hash(payload);
        if (Hash(Read(exe, "msi")) != expected || Encoding.UTF8.GetString(Read(exe, "product_code")) != productCode) {
            throw new InvalidDataException("EXE embedded MSI or uninstall ProductCode does not match the standalone MSI.");
        }
        if (Encoding.UTF8.GetString(Read(exe, "manifest", true)) != manifestText) throw new InvalidDataException("Installer elevation manifest verification failed.");
        return expected;
    }
}
'@
}
$embeddedHash = [V7InstallerResources]::Replace($exe, $msi, $productCode)
[ordered]@{
    exe = $exe
    msi = $msi
    product_code = $productCode
    embedded_msi_sha256 = $embeddedHash
    installer_execution_level = 'requireAdministrator'
    exe_sha256 = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLowerInvariant()
    verified = $true
} | ConvertTo-Json
