[CmdletBinding()]
param(
    [string]$Runtime = "$PSScriptRoot\..\..\.tool-cache\runtime\HLSDownloader",
    [string]$Evidence = "$PSScriptRoot\..\..\artifacts\browser-main-acceptance",
    [string[]]$FunctionalReport = @('firefox-public-adaptive.json','edge-public-adaptive.json','firefox-errors.json','firefox-toolbar.json','firefox-entries.json','real-takeover.json','http-boundaries.json','workbench-controls.json','extension-controls.json'),
    [string]$StartupReport = 'native-startup-final.json'
)
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath("$PSScriptRoot\..\..")
$runtimePath = (Resolve-Path -LiteralPath $Runtime).Path
$evidencePath = (Resolve-Path -LiteralPath $Evidence).Path
if (-not $runtimePath.StartsWith("$repo\", [StringComparison]::OrdinalIgnoreCase) -or
    -not $evidencePath.StartsWith("$repo\", [StringComparison]::OrdinalIgnoreCase)) { throw 'Runtime and evidence must remain in this project.' }
$required = $FunctionalReport
if ($required.Count -eq 0) { throw 'Functional acceptance reports are required.' }
foreach ($name in @($required) + @($StartupReport)) {
    $reportPath = [IO.Path]::GetFullPath((Join-Path $evidencePath $name))
    if (-not $reportPath.StartsWith("$evidencePath\", [StringComparison]::OrdinalIgnoreCase)) { throw 'Acceptance report escaped its evidence directory.' }
    $report = Get-Content -LiteralPath $reportPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($name -ne $StartupReport -and $report.passed -ne $true) { throw "Functional acceptance has not passed: $name" }
}
# 安装后验证独立于性能验收；未达原门槛时仍如实记录，安装包不标记为可发布。
$startup = Get-Content -LiteralPath (Join-Path $evidencePath $StartupReport) -Raw -Encoding UTF8 | ConvertFrom-Json
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
    # 复用随 JRE 分发的运行库，供同目录的 Rust Core/Host/Presenter/Updater 加载。
    Copy-Item -LiteralPath (Join-Path $image 'runtime\bin\vcruntime140.dll') -Destination (Join-Path $image 'app\resources\vcruntime140.dll') -Force
    foreach ($name in @('libmpv-2.dll', 'vulkan-1.dll', 'VulkanLoader-LICENSE.txt', 'NotoSansCJK-OFL.txt')) {
        if (-not (Test-Path -LiteralPath (Join-Path $image "app\resources\$name") -PathType Leaf)) {
            throw "Runtime is missing the bundled player dependency: $name. Rebuild the runtime before packaging."
        }
    }
    # 本机修复记录包含用户任务信息，只保存在工作目录，不进入安装包。
    $localRepair = Join-Path $image 'app\resources\LOCAL-REPAIR.json'
    if (Test-Path -LiteralPath $localRepair) { Remove-Item -LiteralPath $localRepair }
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
        --win-per-user-install --win-dir-chooser --win-shortcut --win-menu --win-menu-group 'HLS Downloader' `
        --win-upgrade-uuid '1c80d5f7-a1ec-4bae-a4a6-e010c5a3ee6b' *> (Join-Path $evidencePath 'installer-build.log')
    if ($LASTEXITCODE -ne 0) { throw "jpackage failed: $LASTEXITCODE" }
    $built = Get-ChildItem -LiteralPath (Join-Path $stage 'delivery') -Filter '*.msi' | Select-Object -First 1
    if (-not $built) { throw 'jpackage did not produce an MSI.' }
    & "$PSScriptRoot\..\build\repack-v7-msi.ps1" -MsiPath $built.FullName -OutMsiPath $final -WixRoot (Join-Path $repo '.tool-cache\build-cache\compose-build\wix311') -ExtensionArchivesDirectory (Join-Path $runtimePath 'extensions') *> (Join-Path $evidencePath 'installer-repack.log')
    $productCode = [guid]::NewGuid().ToString('B').ToUpperInvariant()
    & "$PSScriptRoot\..\build\set-v7-msi-rollback-order.ps1" -MsiPath $final -ProductCode $productCode *> (Join-Path $evidencePath 'installer-lifecycle-patch.log')
    [ordered]@{ built = $true; installed_tested = $false; release_ready = $false; path = $final;
        product_code = $productCode; sha256 = (Get-FileHash -LiteralPath $final -Algorithm SHA256).Hash.ToLowerInvariant();
        user_data_included = $false; functional_reports = $required;
        native_startup_passed = $startup.passed; native_startup_threshold_ms = $startup.threshold_ms;
        native_startup_report = $StartupReport } |
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
