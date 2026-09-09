[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$CandidateManifestPath,
    [string]$ExpectedTvboxHost = $env:HLS_V7_TVBOX_EXPECTED_HOST,
    [string]$InstallDir = 'E:\h',
    [string]$EdgeBinary = $env:HLS_V7_EDGE_BINARY,
    [string]$FirefoxBinary = $env:HLS_V7_FIREFOX_BINARY,
    [string]$EdgeDriver = $env:HLS_V7_EDGE_DRIVER,
    [string]$FirefoxDriver = $env:HLS_V7_FIREFOX_DRIVER,
    [string]$Python = $env:HLS_V7_PYTHON,
    [string]$Ffmpeg = ''
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$utf8NoBom = New-Object Text.UTF8Encoding($false)
$expectedInstallDir = [IO.Path]::GetFullPath('E:\h').TrimEnd('\', '/')
$InstallDir = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\', '/')
if (-not [String]::Equals($InstallDir, $expectedInstallDir, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Browser media-push release gate requires the fixed lifecycle directory ${expectedInstallDir}: $InstallDir"
}
if ([String]::IsNullOrWhiteSpace($ExpectedTvboxHost)) {
    throw 'HLS_V7_TVBOX_EXPECTED_HOST is required; formal media-push validation never skips the real LAN receiver.'
}
$parsedHost = $null
if (-not [Net.IPAddress]::TryParse($ExpectedTvboxHost, [ref]$parsedHost)) {
    throw "HLS_V7_TVBOX_EXPECTED_HOST must be the exact receiver IP address: $ExpectedTvboxHost"
}
$bytes = $parsedHost.GetAddressBytes()
$private = $parsedHost.AddressFamily -eq [Net.Sockets.AddressFamily]::InterNetwork -and (
    $bytes[0] -eq 10 -or
    ($bytes[0] -eq 172 -and $bytes[1] -ge 16 -and $bytes[1] -le 31) -or
    ($bytes[0] -eq 192 -and $bytes[1] -eq 168)
)
if (-not $private) { throw "TVBox receiver must use a private IPv4 LAN address: $ExpectedTvboxHost" }

function Resolve-FullPath([string]$Path, [string]$Base) {
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $Base $Path))
}

function Get-MsiProperty([string]$Path, [string]$Property) {
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

function Get-InstalledProduct([string]$UpgradeCode) {
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

function Invoke-Msi([string[]]$Arguments, [string]$LogPath) {
    $allArguments = @($Arguments) + @('/norestart', 'REBOOT=ReallySuppress', '/l*v', $LogPath)
    $process = Start-Process -FilePath 'msiexec.exe' -ArgumentList $allArguments -Wait -PassThru
    return [int]$process.ExitCode
}

function Get-FreeTcpPort {
    $listener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0)
    try { $listener.Start(); return ([Net.IPEndPoint]$listener.LocalEndpoint).Port }
    finally { $listener.Stop() }
}

function Get-NativeHostRegistration([string]$Parent) {
    if (-not (Test-Path -LiteralPath $Parent)) { return @() }
    return @(
        Get-ChildItem -LiteralPath $Parent |
            Where-Object { $_.PSChildName -eq 'com.ciaooo55.hls_downloader' } |
            ForEach-Object {
                $manifestPath = [string]$_.GetValue('')
                [ordered]@{
                    key = $_.Name
                    host_name = $_.PSChildName
                    manifest = $manifestPath
                    exists = (Test-Path -LiteralPath $manifestPath -PathType Leaf)
                }
            }
    )
}

function Assert-NativeHostRegistration(
    [string]$Parent,
    [string]$Label,
    [string]$AllowlistField,
    [string]$ExpectedAllowlistValue,
    [string]$ExpectedHostPath
) {
    $registrations = @(Get-NativeHostRegistration $Parent)
    if ($registrations.Count -ne 1 -or -not [bool]$registrations[0].exists) {
        throw "Candidate MSI did not install exactly one usable $Label Native Messaging registration: $($registrations | ConvertTo-Json -Compress)"
    }

    $entry = $registrations[0]
    $manifestPath = [IO.Path]::GetFullPath([string]$entry.manifest)
    try {
        $nativeManifest = [IO.File]::ReadAllText($manifestPath, $utf8NoBom) | ConvertFrom-Json
    } catch {
        throw "$Label Native Messaging manifest is not valid UTF-8 JSON: $manifestPath; $($_.Exception.Message)"
    }

    if ([string]$nativeManifest.name -ne 'com.ciaooo55.hls_downloader' -or [string]$nativeManifest.type -ne 'stdio') {
        throw "$Label Native Messaging manifest identity mismatch: $($nativeManifest | ConvertTo-Json -Compress)"
    }
    if ([String]::IsNullOrWhiteSpace([string]$nativeManifest.path)) {
        throw "$Label Native Messaging manifest does not declare a host executable: $manifestPath"
    }

    $actualHostPath = [IO.Path]::GetFullPath([string]$nativeManifest.path)
    $expectedHostFullPath = [IO.Path]::GetFullPath($ExpectedHostPath)
    if (-not [String]::Equals($actualHostPath, $expectedHostFullPath, [StringComparison]::OrdinalIgnoreCase)) {
        throw "$Label Native Messaging manifest is not bound to the installed candidate host: actual=$actualHostPath expected=$expectedHostFullPath"
    }
    if (-not (Test-Path -LiteralPath $actualHostPath -PathType Leaf)) {
        throw "$Label Native Messaging host executable is missing: $actualHostPath"
    }

    $property = $nativeManifest.PSObject.Properties[$AllowlistField]
    if ($null -eq $property) {
        throw "$Label Native Messaging manifest is missing $AllowlistField: $manifestPath"
    }
    $allowlist = @($property.Value)
    if ($allowlist.Count -ne 1 -or [string]$allowlist[0] -ne $ExpectedAllowlistValue) {
        throw "$Label Native Messaging allowlist mismatch for ${AllowlistField}: $($allowlist | ConvertTo-Json -Compress)"
    }

    return [ordered]@{
        key = [string]$entry.key
        host_name = [string]$entry.host_name
        manifest = $manifestPath
        manifest_sha256 = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
        host = $actualHostPath
        host_sha256 = (Get-FileHash -LiteralPath $actualHostPath -Algorithm SHA256).Hash.ToLowerInvariant()
        allowlist_field = $AllowlistField
        allowlist = $allowlist
    }
}

function Assert-Artifact([string]$Root, $Entry, [string]$Label) {
    if ($null -eq $Entry -or [String]::IsNullOrWhiteSpace([string]$Entry.path)) { throw "Candidate manifest is missing $Label." }
    $path = Resolve-FullPath ([string]$Entry.path) $Root
    $rootPrefix = [IO.Path]::GetFullPath($Root).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $path.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw "$Label escaped the candidate root: $path" }
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "$Label is missing: $path" }
    $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -ne ([string]$Entry.sha256).ToLowerInvariant()) { throw "$Label SHA-256 mismatch: $hash" }
    return $path
}

$manifestPath = (Resolve-Path -LiteralPath $CandidateManifestPath).Path
$manifestRoot = Split-Path $manifestPath -Parent
$manifest = [IO.File]::ReadAllText($manifestPath, $utf8NoBom) | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
if ([int]$manifest.schema -ne 1 -or
    [string]$manifest.package_tier -ne 'candidate' -or
    [string]$manifest.source_commit -ne $currentCommit -or
    [string]$manifest.source_tree -ne $currentTree) {
    throw 'Browser media-push release gate requires a candidate manifest from the current source commit/tree.'
}
$candidate = Assert-Artifact $manifestRoot $manifest.artifacts.msi 'candidate MSI'
$chromiumZip = Assert-Artifact $manifestRoot $manifest.extensions.Chromium 'candidate Chromium extension'
$firefoxZip = Assert-Artifact $manifestRoot $manifest.extensions.Firefox 'candidate Firefox extension'
$upgradeCode = Get-MsiProperty $candidate 'UpgradeCode'
if ($upgradeCode -ne '{1C80D5F7-A1EC-4BAE-A4A6-E010C5A3EE6B}') { throw "Candidate MSI UpgradeCode mismatch: $upgradeCode" }
if ($null -ne (Get-InstalledProduct $upgradeCode)) {
    throw 'Browser media-push gate requires a clean dedicated release runner with no HLSDownloader product installed.'
}

$pythonCommand = Get-Command $(if ($Python) { $Python } else { 'python.exe' }) -ErrorAction Stop
$pythonExe = $pythonCommand.Source
$ffmpegExe = if ($Ffmpeg) { (Resolve-Path -LiteralPath $Ffmpeg).Path } elseif ($env:HLS_V7_FFMPEG_DIR) { Join-Path $env:HLS_V7_FFMPEG_DIR 'ffmpeg.exe' } else { (Get-Command ffmpeg.exe -ErrorAction Stop).Source }
if (-not (Test-Path -LiteralPath $ffmpegExe -PathType Leaf)) { throw "FFmpeg is missing: $ffmpegExe" }
$edgeExe = if ($EdgeBinary) { (Resolve-Path -LiteralPath $EdgeBinary).Path } else { '' }
$firefoxExe = if ($FirefoxBinary) { (Resolve-Path -LiteralPath $FirefoxBinary).Path } else { '' }
$edgeDriverPath = if ($EdgeDriver) { (Resolve-Path -LiteralPath $EdgeDriver).Path } else { '' }
$firefoxDriverPath = if ($FirefoxDriver) { (Resolve-Path -LiteralPath $FirefoxDriver).Path } else { '' }

$evidenceRoot = Join-Path $repo 'artifacts\v7-productization\release-evidence\browser-media-push'
$tempRoot = Join-Path $env:RUNNER_TEMP ('hls-v7-browser-media-push-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force -Path $evidenceRoot | Out-Null
New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
$installLog = Join-Path $tempRoot 'install-candidate.log'
$uninstallLog = Join-Path $tempRoot 'uninstall-candidate.log'
$installed = $false
$previousDataDir = $env:HLS_V7_DATA_DIR
$previousDownloadDir = $env:HLS_V7_DOWNLOAD_DIR
$previousTcp = $env:HLS_V7_CORE_TCP
$previousBind = $env:HLS_V7_CORE_BIND
$previousMigrate = $env:HLS_V6_SKIP_MIGRATE
$result = $null
$expectedNativeHost = [IO.Path]::GetFullPath((Join-Path $InstallDir 'app\resources\HLSDownloaderNativeHost.exe'))
$packagedNativeHostInput = Join-Path $repo 'desktop_ui\resources\common\HLSDownloaderNativeHost.exe'
if (-not (Test-Path -LiteralPath $packagedNativeHostInput -PathType Leaf)) {
    throw "Candidate build input Native Messaging host is missing: $packagedNativeHostInput"
}
$packagedNativeHostSha256 = (Get-FileHash -LiteralPath $packagedNativeHostInput -Algorithm SHA256).Hash.ToLowerInvariant()
if (Test-Path -LiteralPath $expectedNativeHost -PathType Leaf) {
    throw "Browser media-push gate requires no pre-existing installed Native Messaging host before candidate MSI install: $expectedNativeHost"
}
$preEdgeRegistration = @(Get-NativeHostRegistration 'HKCU:\Software\Microsoft\Edge\NativeMessagingHosts')
$preFirefoxRegistration = @(Get-NativeHostRegistration 'HKCU:\Software\Mozilla\NativeMessagingHosts')
if ($preEdgeRegistration.Count -gt 0 -or $preFirefoxRegistration.Count -gt 0) {
    throw "Browser media-push gate requires no pre-existing HLS Native Messaging registration before candidate MSI install: edge=$($preEdgeRegistration | ConvertTo-Json -Compress) firefox=$($preFirefoxRegistration | ConvertTo-Json -Compress)"
}
try {
    $installExit = Invoke-Msi @('/i', $candidate, '/qn', "INSTALLDIR=$InstallDir") $installLog
    if ($installExit -notin @(0, 3010, 1641)) { throw "Candidate MSI install failed with exit $installExit" }
    $installed = $true
    $product = Get-InstalledProduct $upgradeCode
    if ($null -eq $product -or [string]$product.version -ne [string]$manifest.product_version) {
        throw "Installed candidate identity mismatch: $($product | ConvertTo-Json -Compress)"
    }

    if (-not (Test-Path -LiteralPath $expectedNativeHost -PathType Leaf)) {
        throw "Installed candidate Native Messaging host is missing: $expectedNativeHost"
    }
    $expectedNativeHostSha256 = (Get-FileHash -LiteralPath $expectedNativeHost -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expectedNativeHostSha256 -ne $packagedNativeHostSha256) {
        throw "Installed Native Messaging host does not match the current candidate build input: installed=$expectedNativeHostSha256 packaged=$packagedNativeHostSha256"
    }
    $edgeRegistration = @(Assert-NativeHostRegistration `
        'HKCU:\Software\Microsoft\Edge\NativeMessagingHosts' `
        'Edge' `
        'allowed_origins' `
        'chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/' `
        $expectedNativeHost)
    $firefoxRegistration = @(Assert-NativeHostRegistration `
        'HKCU:\Software\Mozilla\NativeMessagingHosts' `
        'Firefox' `
        'allowed_extensions' `
        'hls-downloader-store@ciaooo55.com' `
        $expectedNativeHost)
    if ([string]$edgeRegistration[0].host_sha256 -ne $expectedNativeHostSha256 -or
        [string]$firefoxRegistration[0].host_sha256 -ne $expectedNativeHostSha256) {
        throw 'Native Messaging registration host digests do not match the installed candidate host.'
    }

    $chromiumDir = Join-Path $tempRoot 'chromium-extension'
    $firefoxDir = Join-Path $tempRoot 'firefox-extension'
    Expand-Archive -LiteralPath $chromiumZip -DestinationPath $chromiumDir -Force
    Expand-Archive -LiteralPath $firefoxZip -DestinationPath $firefoxDir -Force
    foreach ($extensionDir in @($chromiumDir, $firefoxDir)) {
        if (-not (Test-Path -LiteralPath (Join-Path $extensionDir 'manifest.json') -PathType Leaf)) {
            throw "Candidate extension archive is missing manifest.json after extraction: $extensionDir"
        }
    }

    $corePort = Get-FreeTcpPort
    $isolatedRoot = Join-Path $tempRoot 'runtime-state'
    $env:HLS_V7_DATA_DIR = Join-Path $isolatedRoot 'data'
    $env:HLS_V7_DOWNLOAD_DIR = Join-Path $isolatedRoot 'downloads'
    $env:HLS_V7_CORE_TCP = '1'
    $env:HLS_V7_CORE_BIND = "127.0.0.1:$corePort"
    $env:HLS_V6_SKIP_MIGRATE = '1'
    New-Item -ItemType Directory -Force -Path $env:HLS_V7_DATA_DIR, $env:HLS_V7_DOWNLOAD_DIR | Out-Null

    $launcherPath = Join-Path $InstallDir 'HLSDownloader.exe'
    if (-not (Test-Path -LiteralPath $launcherPath -PathType Leaf)) { throw "Installed launcher is missing: $launcherPath" }
    $launcher = Start-Process -FilePath $launcherPath -WorkingDirectory $InstallDir -PassThru
    $deadline = (Get-Date).AddSeconds(60)
    do {
        Start-Sleep -Milliseconds 250
        $engine = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
            $_.Name -eq 'HLSDownloaderEngine.exe' -and $_.ExecutablePath -and $_.ExecutablePath.StartsWith($InstallDir, [StringComparison]::OrdinalIgnoreCase)
        }) | Select-Object -First 1
    } while (-not $engine -and (Get-Date) -lt $deadline)
    if (-not $engine) { throw 'Installed candidate did not start its Engine.' }

    $accessBridge = Get-ChildItem -LiteralPath $InstallDir -Filter 'WindowsAccessBridge-64.dll' -File -Recurse | Select-Object -First 1
    if (-not $accessBridge) { throw 'Installed candidate runtime is missing WindowsAccessBridge-64.dll.' }

    $reports = [ordered]@{}
    foreach ($browser in @('edge', 'firefox')) {
        $extensionDir = if ($browser -eq 'edge') { $chromiumDir } else { $firefoxDir }
        $reportPath = Join-Path $evidenceRoot "$browser-tvbox-real.json"
        $arguments = @(
            (Join-Path $PSScriptRoot 'smoke_extension_tvbox_real.py'),
            '--browser', $browser,
            '--extension', $extensionDir,
            '--core-port', [string]$corePort,
            '--expected-host', $ExpectedTvboxHost,
            '--ffmpeg', $ffmpegExe,
            '--access-bridge-python', $pythonExe,
            '--access-bridge-dll', $accessBridge.FullName,
            '--output', $reportPath
        )
        $binary = if ($browser -eq 'edge') { $edgeExe } else { $firefoxExe }
        $driver = if ($browser -eq 'edge') { $edgeDriverPath } else { $firefoxDriverPath }
        if ($binary) { $arguments += @('--browser-binary', $binary) }
        if ($driver) { $arguments += @('--driver', $driver) }
        $captured = @(& $pythonExe @arguments 2>&1 | ForEach-Object { $_.ToString() })
        if ($LASTEXITCODE -ne 0) { throw "$browser installed-browser TVBox smoke failed: $($captured -join [Environment]::NewLine)" }
        $report = [IO.File]::ReadAllText($reportPath, $utf8NoBom) | ConvertFrom-Json
        if ($report.passed -ne $true -or [string]$report.expected_receiver_host -ne $ExpectedTvboxHost) {
            throw "$browser TVBox report failed identity validation: $($report | ConvertTo-Json -Compress)"
        }
        $reports[$browser] = [ordered]@{
            path = $reportPath.Substring($repo.Length + 1).Replace('\', '/')
            sha256 = (Get-FileHash -LiteralPath $reportPath -Algorithm SHA256).Hash.ToLowerInvariant()
            receiver_host = [string]$report.expected_receiver_host
            fixture_sha256 = [string]$report.fixture_sha256
        }
    }

    $result = [ordered]@{
        schema = 1
        passed = $true
        product_version = [string]$manifest.product_version
        source_commit = $currentCommit
        source_tree = $currentTree
        candidate_manifest_sha256 = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
        candidate_msi_sha256 = (Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash.ToLowerInvariant()
        installed_product = $product
        native_host_executable = [ordered]@{
            path = $expectedNativeHost
            sha256 = $expectedNativeHostSha256
            packaged_input_path = 'desktop_ui/resources/common/HLSDownloaderNativeHost.exe'
            packaged_input_sha256 = $packagedNativeHostSha256
        }
        edge_registration = $edgeRegistration
        firefox_registration = $firefoxRegistration
        expected_receiver_host = $ExpectedTvboxHost
        browser_reports = $reports
    }
    Write-Output ($result | ConvertTo-Json -Depth 8 -Compress)
} finally {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'shutdown-running.ps1') -InstallDir $InstallDir | Out-Null
    if ($installed) {
        [void](Invoke-Msi @('/x', $candidate, '/qn') $uninstallLog)
    }
    foreach ($name in @('HLS_V7_DATA_DIR','HLS_V7_DOWNLOAD_DIR','HLS_V7_CORE_TCP','HLS_V7_CORE_BIND','HLS_V6_SKIP_MIGRATE')) {
        $previous = switch ($name) {
            'HLS_V7_DATA_DIR' { $previousDataDir }
            'HLS_V7_DOWNLOAD_DIR' { $previousDownloadDir }
            'HLS_V7_CORE_TCP' { $previousTcp }
            'HLS_V7_CORE_BIND' { $previousBind }
            'HLS_V6_SKIP_MIGRATE' { $previousMigrate }
        }
        if ($null -eq $previous -or [String]::IsNullOrEmpty([string]$previous)) { Remove-Item "Env:$name" -ErrorAction SilentlyContinue }
        else { Set-Item "Env:$name" $previous }
    }
    Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
