[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$MsiPath,
    [Parameter(Mandatory = $true)][string]$OutMsiPath,
    [Parameter(Mandatory = $true)][string]$WixRoot,
    [string]$EngineReplacement = '',
    [string]$ProvenanceReplacement = ''
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
    foreach ($replacement in @(
        @('HLSDownloaderEngine.exe', $EngineReplacement),
        @('BUILD-PROVENANCE.json', $ProvenanceReplacement)
    )) {
        if ([string]::IsNullOrWhiteSpace($replacement[1])) { continue }
        $files = $model.SelectNodes("//*[local-name()='File' and @Name='$($replacement[0])']")
        if ($files.Count -ne 1) { throw "Installer replacement file is not unique: $($replacement[0])" }
        $destination = [IO.Path]::GetFullPath((Join-Path $work $files[0].GetAttribute('Source')))
        if (-not $destination.StartsWith($work + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Replacement target escaped the extracted installer.' }
        Copy-Item -LiteralPath (Resolve-Path -LiteralPath $replacement[1]).Path -Destination $destination -Force
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
