[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$AppImage,
    [Parameter(Mandatory=$true)][string]$WixRoot,
    [Parameter(Mandatory=$true)][string]$OutMsi,
    [Parameter(Mandatory=$true)][string]$OutExe,
    [Parameter(Mandatory=$true)][string]$ProductVersion,
    [Parameter(Mandatory=$true)][string]$ProductCode
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$image = (Resolve-Path -LiteralPath $AppImage).Path
$OutMsi = [IO.Path]::GetFullPath($OutMsi)
$OutExe = [IO.Path]::GetFullPath($OutExe)
foreach ($output in @($OutMsi, $OutExe)) {
    if (-not $output.StartsWith($repo.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "Installer output must stay inside the workspace: $output" }
}
$work = Join-Path (Split-Path $OutMsi -Parent) ('.native-installer-' + [guid]::NewGuid().ToString('n'))
if (Test-Path -LiteralPath $work) { throw "Installer work directory already exists: $work" }
New-Item -ItemType Directory -Path $work | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)
function Escape-Xml([string]$Value) { [Security.SecurityElement]::Escape($Value) }
function File-Id([string]$Relative) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try { 'F' + ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Relative.ToLowerInvariant())))).Replace('-','').Substring(0,32) }
    finally { $sha.Dispose() }
}
try {
    $preflight = Join-Path $work 'installer-preflight.exe'
    & rustc.exe --edition 2021 --crate-name hls_installer_preflight -C opt-level=s -C panic=abort -C target-feature=+crt-static (Join-Path $repo 'scripts\install\installer-preflight.rs') -o $preflight
    if ($LASTEXITCODE -ne 0) { throw 'Installer preflight build failed.' }
    $directories = [ordered]@{ '' = 'INSTALLDIR' }
    $components = New-Object 'System.Collections.Generic.List[string]'
    $files = Get-ChildItem -LiteralPath $image -File -Recurse | Sort-Object FullName
    $payload = New-Object Text.StringBuilder
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($image.Length + 1)
        $dir = [IO.Path]::GetDirectoryName($relative)
        $parent = $dir
        while ($parent -and -not $directories.Contains($parent)) {
            $directories[$parent] = 'D' + (File-Id $parent)
            $parent = [IO.Path]::GetDirectoryName($parent)
        }
        $id = File-Id $relative
        $componentGuid = ([guid]::ParseExact($id.Substring(1), 'N')).ToString('B')
        $components.Add($id)
        [void]$payload.AppendLine('<DirectoryRef Id="' + $directories[$dir] + '"><Component Id="C' + $id + '" Guid="' + $componentGuid + '" Win64="yes"><File Id="' + $id + '" Name="' + (Escape-Xml $file.Name) + '" Source="' + (Escape-Xml $file.FullName) + '" /><RegistryValue Root="HKCU" Key="Software\HLS Downloader\Native\Components" Name="' + $id + '" Type="string" Value="[ProductCode]" KeyPath="yes" /></Component></DirectoryRef>')
    }
    # Author parent folders before children; no harvest dependency and no JRE.
    $tree = New-Object Text.StringBuilder
    foreach ($dir in @($directories.Keys | Where-Object { $_ } | Sort-Object Length)) {
        $parent = [IO.Path]::GetDirectoryName($dir)
        if (-not $directories.Contains($parent)) { throw "Missing payload parent directory: $parent" }
        [void]$tree.AppendLine('<DirectoryRef Id="' + $directories[$parent] + '"><Directory Id="' + $directories[$dir] + '" Name="' + (Escape-Xml ([IO.Path]::GetFileName($dir))) + '" /></DirectoryRef>')
    }
    $references = ($components | ForEach-Object { '<ComponentRef Id="C' + $_ + '" />' }) -join "`n"
    $icon = Escape-Xml (Join-Path $repo 'assets\app-icon.ico')
    $license = ([IO.File]::ReadAllText((Join-Path $repo 'LICENSE'))).Replace('\','\\').Replace('{','\{').Replace('}','\}')
    $license = [regex]::Replace($license, '\r?\n', '\par ')
    $licensePath = Join-Path $work 'License.rtf'
    [IO.File]::WriteAllText($licensePath, '{\rtf1\ansi ' + $license + '}', [Text.Encoding]::ASCII)
    $wxs = @"
<?xml version="1.0" encoding="utf-8"?>
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi">
 <Product Id="$ProductCode" Name="HLS Downloader" Language="2052" Version="$ProductVersion" Manufacturer="HLS Downloader" UpgradeCode="1c80d5f7-a1ec-4bae-a4a6-e010c5a3ee6b">
  <Package Id="*" InstallerVersion="500" Compressed="yes" InstallScope="perUser" Platform="x64" Languages="2052" />
  <MediaTemplate EmbedCab="yes" CompressionLevel="high" />
  <Upgrade Id="1c80d5f7-a1ec-4bae-a4a6-e010c5a3ee6b">
   <UpgradeVersion Minimum="0.0.0" IncludeMinimum="yes" Maximum="$ProductVersion" IncludeMaximum="yes" Property="JP_UPGRADABLE_FOUND" />
   <UpgradeVersion Minimum="$ProductVersion" IncludeMinimum="no" OnlyDetect="yes" Property="JP_DOWNGRADABLE_FOUND" />
  </Upgrade>
  <Condition Message="&#x5DF2;&#x5B89;&#x88C5;&#x66F4;&#x65B0;&#x7248;&#x672C;&#xFF0C;&#x4E0D;&#x80FD;&#x964D;&#x7EA7;&#x3002;">NOT JP_DOWNGRADABLE_FOUND</Condition>
  <Property Id="ARPPRODUCTICON" Value="AppIcon" />
  <Icon Id="AppIcon" SourceFile="$icon" />
  <Property Id="V7_SAVED_INSTALLDIR"><RegistrySearch Id="NativeInstallDirSearch" Root="HKCU" Key="Software\HLS Downloader\Native" Name="InstallDir" Type="raw" Win64="yes" /></Property>
  <!-- The published Compose 7.0.3 package persisted custom paths here. -->
  <Property Id="V7_LEGACY_INSTALLDIR"><RegistrySearch Id="LegacyComposeInstallDirSearch" Root="HKCU" Key="Software\HLS Downloader\HLSDownloader\7.0.3" Name="RM_RF48431AECBD69377EA800D62616352F20" Type="raw" Win64="yes" /></Property>
  <Directory Id="TARGETDIR" Name="SourceDir"><Directory Id="LocalAppDataFolder"><Directory Id="INSTALLDIR" Name="HLSDownloader" /></Directory><Directory Id="ProgramMenuFolder"><Directory Id="AppMenu" Name="HLS Downloader" /></Directory></Directory>
  $tree
  $payload
  <DirectoryRef Id="INSTALLDIR"><Component Id="NativeInstallDirectory" Guid="{316F862E-04E5-4142-BB81-0650127923C0}" Win64="yes">
   <RegistryValue Root="HKCU" Key="Software\HLS Downloader\Native" Name="InstallDir" Type="string" Value="[INSTALLDIR]" KeyPath="yes" />
  </Component></DirectoryRef>
  <DirectoryRef Id="AppMenu"><Component Id="NativeShortcuts" Guid="{7574B620-2C3C-42D6-9A8A-30544961A98B}" Win64="yes">
   <Shortcut Id="StartMenuShortcut" Name="HLS Downloader" Target="[INSTALLDIR]HLSDownloader.exe" WorkingDirectory="INSTALLDIR" Icon="AppIcon" />
   <RegistryValue Root="HKCU" Key="Software\HLS Downloader\Native" Name="StartMenu" Type="integer" Value="1" KeyPath="yes" /><RemoveFolder Id="RemoveAppMenu" On="uninstall" />
  </Component></DirectoryRef>
  <Feature Id="ProductFeature" Title="HLS Downloader" Level="1">$references<ComponentRef Id="NativeInstallDirectory" /><ComponentRef Id="NativeShortcuts" /></Feature>
  <Binary Id="V7InstallerHelper" SourceFile="$(Escape-Xml $preflight)" />
  <CustomAction Id="V7StopRunningApplications" BinaryKey="V7InstallerHelper" ExeCommand="&quot;[V7_EXISTING_INSTALLDIR]\.&quot;" Return="check" />
  <InstallExecuteSequence><RemoveExistingProducts After="InstallInitialize" /><Custom Action="V7StopRunningApplications" Before="InstallValidate">V7_EXISTING_INSTALLDIR AND (JP_UPGRADABLE_FOUND OR Installed) AND NOT UPGRADINGPRODUCTCODE</Custom></InstallExecuteSequence>
  <UIRef Id="WixUI_InstallDir" /><Property Id="WIXUI_INSTALLDIR" Value="INSTALLDIR" /><WixVariable Id="WixUILicenseRtf" Value="$(Escape-Xml $licensePath)" />
 </Product>
</Wix>
"@
    $source = Join-Path $work 'Product.wxs'
    [IO.File]::WriteAllText($source, $wxs, $utf8)
    & (Join-Path $WixRoot 'candle.exe') $source -o (Join-Path $work 'Product.wixobj') -nologo
    if ($LASTEXITCODE -ne 0) { throw 'Native MSI compilation failed.' }
    & (Join-Path $WixRoot 'light.exe') (Join-Path $work 'Product.wixobj') -out $OutMsi -pdbout (Join-Path $work 'Product.wixpdb') -nologo -cultures:zh-CN -sice:ICE64 -sice:ICE91 -ext (Join-Path $WixRoot 'WixUIExtension.dll')
    if ($LASTEXITCODE -ne 0) { throw 'Native MSI link failed.' }
    & (Join-Path $PSScriptRoot 'set-v7-msi-rollback-order.ps1') -MsiPath $OutMsi -ProductCode $ProductCode
    if ($LASTEXITCODE -ne 0) { throw 'Native MSI lifecycle authoring failed.' }
    $bundle = @"
<?xml version="1.0" encoding="utf-8"?>
<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi" xmlns:bal="http://schemas.microsoft.com/wix/BalExtension">
 <Bundle Name="HLS Downloader" Version="$ProductVersion" Manufacturer="HLS Downloader" UpgradeCode="D327B622-48B2-465D-A71B-07F6EF4B79D9" IconSourceFile="$icon">
  <BootstrapperApplicationRef Id="WixStandardBootstrapperApplication.HyperlinkLicense"><bal:WixStandardBootstrapperApplication LicenseUrl="https://github.com/ciaooo55/hls-downloader/blob/main/LICENSE" /></BootstrapperApplicationRef>
  <Chain><MsiPackage SourceFile="$(Escape-Xml ([IO.Path]::GetFullPath($OutMsi)))" DisplayInternalUI="yes" Compressed="yes" Vital="yes" /></Chain>
 </Bundle>
</Wix>
"@
    $source = Join-Path $work 'Bundle.wxs'
    [IO.File]::WriteAllText($source, $bundle, $utf8)
    & (Join-Path $WixRoot 'candle.exe') $source -o (Join-Path $work 'Bundle.wixobj') -nologo -ext (Join-Path $WixRoot 'WixBalExtension.dll')
    if ($LASTEXITCODE -ne 0) { throw 'Native bundle compilation failed.' }
    & (Join-Path $WixRoot 'light.exe') (Join-Path $work 'Bundle.wixobj') -out $OutExe -pdbout (Join-Path $work 'Bundle.wixpdb') -nologo -ext (Join-Path $WixRoot 'WixBalExtension.dll')
    if ($LASTEXITCODE -ne 0) { throw 'Native bundle link failed.' }
    $complete = $true
} finally {
    # Retain failed authoring evidence; remove only after the entire pair exists.
    if ($complete) {
        Remove-Item -LiteralPath $work -Recurse -Force
    }
}
