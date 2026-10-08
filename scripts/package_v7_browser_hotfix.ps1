[CmdletBinding()]
param(
    [string]$Runtime = "$PSScriptRoot\..\.tool-cache\runtime\HLSDownloader",
    [string]$Evidence = "$PSScriptRoot\..\artifacts\browser-main-acceptance"
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath("$PSScriptRoot\..")
$runtimePath = (Resolve-Path -LiteralPath $Runtime).Path
$evidencePath = (Resolve-Path -LiteralPath $Evidence).Path
if (-not $runtimePath.StartsWith("$repo\", [StringComparison]::OrdinalIgnoreCase) -or
    -not $evidencePath.StartsWith("$repo\", [StringComparison]::OrdinalIgnoreCase)) { throw 'Runtime and evidence must remain in this project.' }
$required = @('firefox-public-adaptive','edge-public-adaptive','firefox-errors','firefox-toolbar','firefox-entries','real-takeover','http-boundaries','workbench-controls','extension-controls')
foreach ($name in $required) {
    $report = Get-Content -LiteralPath (Join-Path $evidencePath "$name.json") -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($report.passed -ne $true) { throw "Functional acceptance has not passed: $name" }
}
# 安装后验证独立于性能验收；未达原门槛时仍如实记录，安装包不标记为可发布。
$startup = Get-Content -LiteralPath (Join-Path $evidencePath 'native-startup-final.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$output = Join-Path $evidencePath 'delivery'
New-Item -ItemType Directory -Force -Path $output | Out-Null
$final = Join-Path $output 'HLSDownloader-7.0.3-browser-fix-Windows-x64.msi'
if (Test-Path -LiteralPath $final) { throw 'Installer already exists; do not silently overwrite a tested package.' }
$stage = Join-Path $repo ('.tool-cache\test-tmp\browser-installer-' + [guid]::NewGuid().ToString('N'))
$image = Join-Path $stage 'image\HLSDownloader'
New-Item -ItemType Directory -Force -Path $image | Out-Null
$drive = $null
$savedPath = $env:PATH
try {
    foreach ($name in @('app','runtime','extensions')) {
        Copy-Item -LiteralPath (Join-Path $runtimePath $name) -Destination $image -Recurse
    }
    Copy-Item -LiteralPath (Join-Path $runtimePath 'HLSDownloader.exe') -Destination $image
    # 安装在用户可写目录；数据留在安装目录，不包含当前用户数据库和下载文件。
    New-Item -ItemType File -Path (Join-Path $image 'portable') | Out-Null
    foreach ($letter in @('Z','Y','X','W')) {
        if (Test-Path "${letter}:\") { continue }
        & subst.exe "${letter}:" $stage
        if ($LASTEXITCODE -eq 0) { $drive = "${letter}:"; break }
    }
    if (-not $drive) { throw 'No free drive letter for the installer build path.' }
    $env:PATH = (Join-Path $repo '.tool-cache\build-cache\compose-build\wix311') + ';' + $savedPath
    $jdk = Join-Path $repo '.tool-cache\build-cache\jdk-21\bin\jpackage.exe'
    & $jdk --type msi --name HLSDownloader --app-version 7.0.3 --vendor 'HLS Downloader' `
        --app-image "$drive\image\HLSDownloader" --dest "$drive\delivery" --temp "$drive\tmp" `
        --win-per-user-install --win-shortcut --win-menu --win-menu-group 'HLS Downloader' `
        --win-upgrade-uuid '1c80d5f7-a1ec-4bae-a4a6-e010c5a3ee6b' *> (Join-Path $evidencePath 'installer-build.log')
    if ($LASTEXITCODE -ne 0) { throw "jpackage failed: $LASTEXITCODE" }
    $built = Get-ChildItem -LiteralPath (Join-Path $stage 'delivery') -Filter '*.msi' | Select-Object -First 1
    if (-not $built) { throw 'jpackage did not produce an MSI.' }
    & "$PSScriptRoot\repack-v7-msi.ps1" -MsiPath $built.FullName -OutMsiPath $final -WixRoot (Join-Path $repo '.tool-cache\build-cache\compose-build\wix311') *> (Join-Path $evidencePath 'installer-repack.log')
    $productCode = [guid]::NewGuid().ToString('B').ToUpperInvariant()
    & "$PSScriptRoot\set-v7-msi-rollback-order.ps1" -MsiPath $final -ProductCode $productCode *> (Join-Path $evidencePath 'installer-lifecycle-patch.log')
    [ordered]@{ built = $true; installed_tested = $false; release_ready = $false; path = $final;
        product_code = $productCode; sha256 = (Get-FileHash -LiteralPath $final -Algorithm SHA256).Hash.ToLowerInvariant();
        user_data_included = $false; functional_reports = $required;
        native_startup_passed = $startup.passed; native_startup_threshold_ms = $startup.threshold_ms;
        native_startup_report = 'native-startup-final.json' } |
        ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $evidencePath 'installer.json') -Encoding UTF8
    Write-Output $final
} finally {
    $env:PATH = $savedPath
    if ($drive) { & subst.exe $drive /d }
    $resolvedStage = [IO.Path]::GetFullPath($stage)
    if ($resolvedStage.StartsWith((Join-Path $repo '.tool-cache\test-tmp\'), [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $resolvedStage -Recurse -Force -ErrorAction SilentlyContinue
    }
}
