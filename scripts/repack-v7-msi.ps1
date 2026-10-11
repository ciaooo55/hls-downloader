[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$MsiPath,
    [Parameter(Mandatory = $true)][string]$OutMsiPath,
    [Parameter(Mandatory = $true)][string]$WixRoot,
    [string]$EngineReplacement = '',
    [string]$NativeHostReplacement = '',
    [string]$PresenterReplacement = '',
    [string]$ProvenanceReplacement = '',
    [string]$ExtensionArchivesDirectory = '',
    [string]$NativeDependenciesDirectory = ''
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
        @('BUILD-PROVENANCE.json', $ProvenanceReplacement)
    )) {
        if ([string]::IsNullOrWhiteSpace($replacement[1])) { continue }
        $files = $model.SelectNodes("//*[local-name()='File' and @Name='$($replacement[0])']")
        if ($files.Count -ne 1) { throw "Installer replacement file is not unique: $($replacement[0])" }
        $destination = [IO.Path]::GetFullPath((Join-Path $work $files[0].GetAttribute('Source')))
        if (-not $destination.StartsWith($work + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Replacement target escaped the extracted installer.' }
        Copy-Item -LiteralPath (Resolve-Path -LiteralPath $replacement[1]).Path -Destination $destination -Force
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
    Copy-Item -LiteralPath $built -Destination $out -Force
    [ordered]@{ input = $msi; output = $out; size = (Get-Item -LiteralPath $out).Length; repacked = $true } | ConvertTo-Json
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
