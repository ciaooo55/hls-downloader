[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$MsiPath,
    [Parameter(Mandatory = $true)][string]$OutMsiPath,
    [Parameter(Mandatory = $true)][string]$WixRoot,
    [string]$EngineReplacement = '',
    [string]$NativeHostReplacement = '',
    [string]$PresenterReplacement = '',
    [string]$UpdaterReplacement = '',
    [string]$DesktopJarReplacement = '',
    [string]$ProvenanceReplacement = '',
    [string]$ExtensionArchivesDirectory = '',
    [string]$NativeDependenciesDirectory = '',
    [switch]$OmitTestAndUnusedIcons
)

$ErrorActionPreference = 'Stop'
$msi = (Resolve-Path -LiteralPath $MsiPath).Path
$out = [IO.Path]::GetFullPath($OutMsiPath)
$work = Join-Path ([IO.Path]::GetDirectoryName($out)) ('.repack-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
    & (Join-Path $WixRoot 'dark.exe') $msi -x $work -o (Join-Path $work 'HLSDownloader.wxs') -nologo *> (Join-Path $work 'dark.log')
    if ($LASTEXITCODE -ne 0) { throw "WiX dark failed: $LASTEXITCODE" }
    $wxs = Join-Path $work 'HLSDownloader.wxs'
    $text = [IO.File]::ReadAllText($wxs, [Text.Encoding]::UTF8)
    # WiX 3's default UI library is codepage 1252. Keep the installer's UI
    # ASCII-safe while all application-facing text remains in the packaged app.
    $text = [regex]::Replace($text, '[^\x00-\x7F]', '')
    $text = $text.Replace('Title=""', 'Title="Main Features"')
    $text = [regex]::Replace($text, 'Source="[^"]*\\(File|Binary|Icon)\\', 'Source="$1\\')
    $text = [regex]::Replace($text, 'SourceFile="[^"]*\\(File|Binary|Icon)\\', 'SourceFile="$1\\')
    $text = [regex]::Replace($text, '<File\b(?![^>]*\bCompressed=)', '<File Compressed="yes"')
    [IO.File]::WriteAllText($wxs, $text, (New-Object Text.UTF8Encoding($false)))
    # 修复单个 Core 时复用已验收安装包的其他文件，不再构建整套应用镜像。
    [xml]$model = $text
    $namespace = $model.DocumentElement.NamespaceURI
    $product = $model.Wix.Product
    # jpackage 的包可能已有向导，但缺少间接目录控件依赖的属性；不能只在新增向导时补。
    if ($model.SelectNodes("//*[local-name()='Directory' and @Id='INSTALLDIR']").Count -ne 1) {
        throw 'Installer INSTALLDIR directory is not unique.'
    }
    $property = $model.SelectSingleNode("//*[local-name()='Property' and @Id='WIXUI_INSTALLDIR']")
    if ($null -eq $property) {
        $property = $model.CreateElement('Property', $namespace)
        $property.SetAttribute('Id', 'WIXUI_INSTALLDIR')
        $product.AppendChild($property) | Out-Null
    }
    $property.SetAttribute('Value', 'INSTALLDIR')
    $license = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../../LICENSE'))
    $license = $license.Replace('\', '\\').Replace('{', '\{').Replace('}', '\}')
    $license = [regex]::Replace($license, '\r?\n', '\par ')
    $licensePath = Join-Path $work 'License.rtf'
    [IO.File]::WriteAllText($licensePath, '{\rtf1\ansi\deff0 {\fonttbl {\f0 Segoe UI;}}\f0\fs20 ' + $license + '}', [Text.Encoding]::ASCII)
    # 部分旧基础包没有安装向导；补上目录选择，不能只支持命令行 INSTALLDIR。
    if ($model.SelectNodes("//*[local-name()='Dialog']").Count -eq 0 -and
        $null -eq $model.SelectSingleNode("//*[local-name()='UIRef' and @Id='WixUI_InstallDir']")) {
        $ui = $model.CreateElement('UIRef', $namespace)
        $ui.SetAttribute('Id', 'WixUI_InstallDir')
        $product.AppendChild($ui) | Out-Null
    }
    $control = $model.SelectSingleNode("//*[local-name()='Dialog' and @Id='LicenseAgreementDlg']/*[local-name()='Control' and @Id='LicenseText']")
    if ($null -ne $control) {
        # 许可正文已展开到 Control 时，WixUILicenseRtf 不能覆盖这份文本。
        $control.RemoveAttribute('Text')
        foreach ($node in @($control.SelectNodes("./*[local-name()='Text']"))) {
            $control.RemoveChild($node) | Out-Null
        }
        $licenseText = $model.CreateElement('Text', $namespace)
        $licenseText.SetAttribute('SourceFile', $licensePath)
        $control.PrependChild($licenseText) | Out-Null
    } else {
        # dark 会将标准向导还原为 UIRef，但保留 jpackage 自定义对话框。
        if ($null -eq $model.SelectSingleNode("//*[local-name()='UIRef' and @Id='WixUI_InstallDir']")) {
            throw 'Installer license dialog or WixUI_InstallDir reference is missing.'
        }
        $variable = $model.SelectSingleNode("//*[local-name()='WixVariable' and @Id='WixUILicenseRtf']")
        if ($null -eq $variable) {
            $variable = $model.CreateElement('WixVariable', $namespace)
            $variable.SetAttribute('Id', 'WixUILicenseRtf')
            $product.AppendChild($variable) | Out-Null
        }
        $variable.SetAttribute('Value', $licensePath)
    }
    # 标准 FindRelatedProducts 已支持三段版本；jpackage 自定义检测会错误清空已找到的旧产品。
    foreach ($node in @($model.SelectNodes("//*[local-name()='Custom' and @Action='JpFindRelatedProducts'] | //*[local-name()='CustomAction' and @Id='JpFindRelatedProducts']"))) {
        $node.ParentNode.RemoveChild($node) | Out-Null
    }
    if ($model.SelectNodes("//*[local-name()='CustomAction' and @BinaryKey='JpCaDll']").Count -eq 0) {
        foreach ($node in @($model.SelectNodes("//*[local-name()='Binary' and @Id='JpCaDll']"))) {
            $node.ParentNode.RemoveChild($node) | Out-Null
        }
    }
    $model.Save($wxs)
    $engineFile = $model.SelectNodes("//*[local-name()='File' and @Name='HLSDownloaderEngine.exe']")
    if ($engineFile.Count -ne 1) { throw 'Installer Engine file is not unique.' }
    $nativeDirectory = $engineFile[0].ParentNode.ParentNode
    if ($nativeDirectory.LocalName -notin @('Directory', 'DirectoryRef')) { throw 'Unexpected native installer directory.' }
    if ($nativeDirectory.SelectNodes("./*[local-name()='Component']/*[local-name()='File' and @Name='vcruntime140.dll']").Count -eq 0) {
        $jreRuntime = $model.SelectNodes("//*[local-name()='File' and @Name='vcruntime140.dll']")
        if ($jreRuntime.Count -ne 1) { throw 'The bundled JRE MSVC runtime file is not unique.' }
        $namespace = $model.DocumentElement.NamespaceURI
        $file = $model.CreateElement('File', $namespace)
        $file.SetAttribute('Id', 'V7NativeVCRuntime140File')
        $file.SetAttribute('Name', 'vcruntime140.dll')
        $file.SetAttribute('Source', $jreRuntime[0].GetAttribute('Source'))
        $file.SetAttribute('KeyPath', 'no')
        $file.SetAttribute('Compressed', 'yes')
        # 依赖跟随 Core 安装，沿用它现有的 per-user HKCU KeyPath。
        $engineFile[0].ParentNode.AppendChild($file) | Out-Null
        $model.Save($wxs)
    }
    if (-not [string]::IsNullOrWhiteSpace($NativeDependenciesDirectory)) {
        $dependencies = (Resolve-Path -LiteralPath $NativeDependenciesDirectory).Path
        $dependencyNames = @('vulkan-1.dll', 'VulkanLoader-LICENSE.txt')
        if (Test-Path -LiteralPath (Join-Path $dependencies 'NotoSansCJK-OFL.txt') -PathType Leaf) { $dependencyNames += 'NotoSansCJK-OFL.txt' }
        foreach ($name in $dependencyNames) {
            $source = Join-Path $dependencies $name
            if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing native dependency: $name" }
            if ($nativeDirectory.SelectNodes("./*[local-name()='Component']/*[local-name()='File' and @Name='$name']").Count -ne 0) { throw "Native dependency is already packaged: $name" }
            $file = $model.CreateElement('File', $model.DocumentElement.NamespaceURI)
            $file.SetAttribute('Id', 'V7Dependency_' + $name.Replace('.', '_').Replace('-', '_'))
            $file.SetAttribute('Name', $name)
            $file.SetAttribute('Source', $source)
            $file.SetAttribute('KeyPath', 'no')
            $file.SetAttribute('Compressed', 'yes')
            $engineFile[0].ParentNode.AppendChild($file) | Out-Null
        }
        $model.Save($wxs)
    }
    foreach ($replacement in @(
        @('HLSDownloaderEngine.exe', $EngineReplacement),
        @('HLSDownloaderNativeHost.exe', $NativeHostReplacement),
        @('HLSDownloaderPresenter.exe', $PresenterReplacement),
        @('HLSDownloaderUpdater.exe', $UpdaterReplacement),
        @('HLSDownloaderDesktop-7.0.3.jar', $DesktopJarReplacement),
        @('BUILD-PROVENANCE.json', $ProvenanceReplacement)
    )) {
        if ([string]::IsNullOrWhiteSpace($replacement[1])) { continue }
        $files = $model.SelectNodes("//*[local-name()='File' and @Name='$($replacement[0])']")
        if ($files.Count -ne 1) { throw "Installer replacement file is not unique: $($replacement[0])" }
        $destination = [IO.Path]::GetFullPath((Join-Path $work $files[0].GetAttribute('Source')))
        if (-not $destination.StartsWith($work + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Replacement target escaped the extracted installer.' }
        Copy-Item -LiteralPath (Resolve-Path -LiteralPath $replacement[1]).Path -Destination $destination -Force
    }
    if ($OmitTestAndUnusedIcons) {
        if ([string]::IsNullOrWhiteSpace($DesktopJarReplacement)) { throw 'Slim repacking requires the rebuilt desktop JAR.' }
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $jar = [IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath $DesktopJarReplacement).Path)
        try {
            foreach ($name in @('OutlinedIconsKt', 'AutoMirroredIconsKt')) {
                if (-not $jar.GetEntry("com/hlsdownloader/desktop/icons/$name.class")) { throw "Rebuilt desktop JAR lacks $name." }
            }
        } finally { $jar.Dispose() }
        foreach ($file in @($model.SelectNodes("//*[local-name()='File' and (@Name='ffprobe.exe' or starts-with(@Name, 'material-icons-extended-desktop-'))]"))) {
            $component = $file.ParentNode
            $component.RemoveChild($file) | Out-Null
            if ($component.SelectNodes("./*[local-name()='File']").Count -eq 0) {
                $id = $component.GetAttribute('Id')
                foreach ($reference in @($model.SelectNodes("//*[local-name()='ComponentRef' and @Id='$id']"))) {
                    $reference.ParentNode.RemoveChild($reference) | Out-Null
                }
                $component.ParentNode.RemoveChild($component) | Out-Null
            }
        }
        $configs = $model.SelectNodes("//*[local-name()='File' and @Name='HLSDownloader.cfg']")
        if ($configs.Count -ne 1) { throw 'Desktop launcher configuration is not unique.' }
        $config = [IO.Path]::GetFullPath((Join-Path $work $configs[0].GetAttribute('Source')))
        if (-not $config.StartsWith($work + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Launcher configuration escaped extracted installer.' }
        $lines = [IO.File]::ReadAllLines($config) | Where-Object { $_ -notmatch '^app\.classpath=.*material-icons-extended-desktop-' }
        [IO.File]::WriteAllLines($config, [string[]]$lines, (New-Object Text.UTF8Encoding($false)))
        $model.Save($wxs)
    }
    if (-not [string]::IsNullOrWhiteSpace($ExtensionArchivesDirectory)) {
        $archives = (Resolve-Path -LiteralPath $ExtensionArchivesDirectory).Path
        foreach ($file in $model.SelectNodes("//*[local-name()='File']")) {
            $name = $file.GetAttribute('Name')
            if ($name -notmatch '^HLSDownloader-.*-(Firefox|Chromium)\.zip$') { continue }
            $family = $Matches[1]
            $replacement = Join-Path $archives "$family.zip"
            if (-not (Test-Path -LiteralPath $replacement -PathType Leaf)) { throw "Missing current extension archive: $family.zip" }
            $destination = [IO.Path]::GetFullPath((Join-Path $work $file.GetAttribute('Source')))
            if (-not $destination.StartsWith($work + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Archive target escaped extracted installer.' }
            Copy-Item -LiteralPath $replacement -Destination $destination -Force
        }
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        foreach ($family in @('Firefox', 'Chromium')) {
            $archivePath = Join-Path $archives "$family.zip"
            if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) { throw "Missing current extension archive: $family.zip" }
            $familyDirectories = $model.SelectNodes("//*[local-name()='Directory' and @Name='$family']")
            if ($familyDirectories.Count -ne 1) { throw "Installer extension directory is not unique: $family" }
            $familyDirectory = $familyDirectories[0]
            $installerFiles = @{}
            foreach ($file in $familyDirectory.SelectNodes(".//*[local-name()='File']")) {
                $parts = [Collections.Generic.List[string]]::new()
                $parts.Insert(0, $file.GetAttribute('Name'))
                $parent = $file.ParentNode
                while ($parent -and -not [object]::ReferenceEquals($parent, $familyDirectory)) {
                    if ($parent.LocalName -eq 'Directory') { $parts.Insert(0, $parent.GetAttribute('Name')) }
                    $parent = $parent.ParentNode
                }
                if (-not [object]::ReferenceEquals($parent, $familyDirectory)) { throw "Installer extension file escaped its directory: $family/$($file.GetAttribute('Name'))" }
                $relativePath = $parts -join '/'
                if ($installerFiles.ContainsKey($relativePath)) { throw "Duplicate installer extension path: $family/$relativePath" }
                $installerFiles[$relativePath] = $file
            }
            $archive = [IO.Compression.ZipFile]::OpenRead((Resolve-Path -LiteralPath $archivePath).Path)
            try {
                $archiveFiles = @{}
                foreach ($entry in $archive.Entries) {
                    if ($entry.FullName.EndsWith('/')) { continue }
                    if ($archiveFiles.ContainsKey($entry.FullName)) { throw "Duplicate ZIP extension path: $family/$($entry.FullName)" }
                    $archiveFiles[$entry.FullName] = $entry
                }
                $installerOnly = @($installerFiles.Keys | Where-Object { -not $archiveFiles.ContainsKey($_) })
                $archiveOnly = @($archiveFiles.Keys | Where-Object { -not $installerFiles.ContainsKey($_) })
                if ($installerFiles.Count -ne $archiveFiles.Count) {
                    throw "Extension file count mismatch for $family`: MSI=$($installerFiles.Count), ZIP=$($archiveFiles.Count); MSI-only=[$($installerOnly -join ', ')]; ZIP-only=[$($archiveOnly -join ', ')]."
                }
                $mappedFiles = @{}
                foreach ($path in $archiveFiles.Keys) {
                    if ($installerFiles.ContainsKey($path)) {
                        $mappedFiles[$path] = $installerFiles[$path]
                        continue
                    }
                    $newLeaf = [IO.Path]::GetFileName($path)
                    $newDirectory = [IO.Path]::GetDirectoryName($path)
                    $newMatch = [regex]::Match($newLeaf, '^(?<stem>.+)-(?<hash>[A-Za-z0-9_-]{8,})\.(?<extension>js|css)$')
                    $candidates = @()
                    if ($newMatch.Success) {
                        foreach ($oldPath in $installerOnly) {
                            if ([IO.Path]::GetDirectoryName($oldPath) -ne $newDirectory) { continue }
                            $oldMatch = [regex]::Match([IO.Path]::GetFileName($oldPath), '^(?<stem>.+)-(?<hash>[A-Za-z0-9_-]{8,})\.(?<extension>js|css)$')
                            if ($oldMatch.Success -and $oldMatch.Groups['stem'].Value -eq $newMatch.Groups['stem'].Value -and $oldMatch.Groups['extension'].Value -eq $newMatch.Groups['extension'].Value) {
                                $candidates += $oldPath
                            }
                        }
                    }
                    if ($candidates.Count -ne 1) { throw "Extension ZIP path mismatch for $family`: ZIP-only='$path'; MSI-only=[$($installerOnly -join ', ')]; no unique same-directory hashed asset match." }
                    $oldPath = $candidates[0]
                    $mappedFiles[$path] = $installerFiles[$oldPath]
                    $installerOnly = @($installerOnly | Where-Object { $_ -ne $oldPath })
                }
                if ($installerOnly.Count) {
                    $oldPaths = $installerOnly -join ', '
                    throw "Extension ZIP path mismatch for $family`: MSI-only=[$oldPaths]; ZIP-only paths could not be paired with a same-directory hashed asset."
                }
                foreach ($path in $archiveFiles.Keys) {
                    $file = $mappedFiles[$path]
                    $relativeSource = $file.GetAttribute('Source')
                    $oldDestination = [IO.Path]::GetFullPath((Join-Path $work $relativeSource))
                    if (-not $oldDestination.StartsWith($work + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "Extension file target escaped the extracted installer: $family/$path" }
                    $newName = [IO.Path]::GetFileName($path)
                    if ($file.GetAttribute('Name') -ne $newName) {
                        $newSource = Join-Path "SourceDir\File\$family" ($path.Replace('/', '\'))
                        $destination = [IO.Path]::GetFullPath((Join-Path $work $newSource))
                        if (-not $destination.StartsWith($work + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "Extension file target escaped the extracted installer: $family/$path" }
                        New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($destination)) | Out-Null
                        [IO.Compression.ZipFileExtensions]::ExtractToFile($archiveFiles[$path], $destination, $true)
                        $file.SetAttribute('Name', $newName)
                        $file.SetAttribute('Source', $newSource)
                        $file.RemoveAttribute('ShortName')
                        $sourceStillUsed = $false
                        foreach ($otherFile in $model.SelectNodes("//*[local-name()='File']")) {
                            if ([object]::ReferenceEquals($otherFile, $file)) { continue }
                            if ($otherFile.GetAttribute('Source') -eq $relativeSource) { $sourceStillUsed = $true; break }
                        }
                        if (-not $sourceStillUsed) { Remove-Item -LiteralPath $oldDestination -Force }
                    } else {
                        [IO.Compression.ZipFileExtensions]::ExtractToFile($archiveFiles[$path], $oldDestination, $true)
                    }
                }
            } finally { $archive.Dispose() }
        }
        $model.Save($wxs)
    }
    $wixobj = Join-Path $work 'HLSDownloader.wixobj'
    & (Join-Path $WixRoot 'candle.exe') $wxs -o $wixobj -nologo -ext (Join-Path $WixRoot 'WixUtilExtension.dll')
    if ($LASTEXITCODE -ne 0) { throw "WiX candle failed: $LASTEXITCODE" }
    $built = Join-Path $work 'repacked.msi'
    # jpackage emits RemoveExistingProducts before the final lifecycle patch
    # moves it to 1510. Suppress that transient ICE27 finding here; the patch
    # script verifies the persisted sequence after repacking.
    & (Join-Path $WixRoot 'light.exe') $wixobj -out $built -b $work -nologo -sice:ICE27 -sice:ICE64 -sice:ICE91 -ext (Join-Path $WixRoot 'WixUtilExtension.dll') -ext (Join-Path $WixRoot 'WixUIExtension.dll')
    if ($LASTEXITCODE -ne 0) { throw "WiX light failed: $LASTEXITCODE" }
    # 静默安装不会初始化向导控件；在交付前检查最终 MSI 中的目录绑定及完整许可正文。
    $installer = $null
    $database = $null
    try {
        $installer = New-Object -ComObject WindowsInstaller.Installer
        $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @([string]$built, 0))
        foreach ($check in @(
            @("SELECT ``Value`` FROM ``Property`` WHERE ``Property``='WIXUI_INSTALLDIR'", 'INSTALLDIR'),
            @("SELECT ``Text`` FROM ``Control`` WHERE ``Dialog_``='LicenseAgreementDlg' AND ``Control``='LicenseText'", [IO.File]::ReadAllText($licensePath))
        )) {
            $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @([string]$check[0]))
            $record = $null
            try {
                $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
                $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
                if ($null -eq $record) { throw "Installer UI assertion has no row: $($check[0])" }
                $actual = $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, 1)
                if ($actual -cne $check[1]) { throw "Installer UI assertion failed: $($check[0])" }
            } finally {
                if ($null -ne $record) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) | Out-Null }
                [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) | Out-Null
            }
        }
    } finally {
        if ($null -ne $database) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) | Out-Null }
        if ($null -ne $installer) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null }
    }
    Copy-Item -LiteralPath $built -Destination $out -Force
    [ordered]@{ input = $msi; output = $out; size = (Get-Item -LiteralPath $out).Length; repacked = $true } | ConvertTo-Json
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
