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
& (Join-Path $PSScriptRoot 'assert-v7-interactive-runner.ps1') | Out-Host
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
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
    $all = @($Arguments) + @('/norestart', 'REBOOT=ReallySuppress', '/l*v', $LogPath)
    $process = Start-Process -FilePath 'msiexec.exe' -ArgumentList $all -Wait -PassThru
    return [int]$process.ExitCode
}

function Get-FreePort {
    $listener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0)
    try { $listener.Start(); return ([Net.IPEndPoint]$listener.LocalEndpoint).Port }
    finally { $listener.Stop() }
}

function Resolve-Browser([string]$Explicit, [string[]]$Defaults, [string]$Label) {
    if (-not [String]::IsNullOrWhiteSpace($Explicit)) {
        $resolved = (Resolve-Path -LiteralPath $Explicit).Path
        if (Test-Path -LiteralPath $resolved -PathType Leaf) { return $resolved }
    }
    foreach ($candidate in $Defaults) {
        if (-not [String]::IsNullOrWhiteSpace($candidate) -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    throw "$Label browser binary was not found."
}

function Get-NativeRegistration([string]$Root, [string]$ExpectedHost) {
    $paths = @(
        "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\$ExpectedHost",
        "HKCU:\Software\Google\Chrome\NativeMessagingHosts\$ExpectedHost",
        "HKCU:\Software\Mozilla\NativeMessagingHosts\$ExpectedHost"
    )
    return @($paths | ForEach-Object {
        if (-not (Test-Path -LiteralPath $_)) { return }
        $manifestPath = [string](Get-ItemPropertyValue -LiteralPath $_ -Name '(default)' -ErrorAction SilentlyContinue)
        if ([String]::IsNullOrWhiteSpace($manifestPath) -or -not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { return }
        $manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
        $hostPath = [string]$manifest.path
        if (-not [IO.Path]::IsPathRooted($hostPath)) { $hostPath = Join-Path (Split-Path $manifestPath -Parent) $hostPath }
        $hostPath = [IO.Path]::GetFullPath($hostPath)
        if (-not $hostPath.StartsWith($Root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Native Messaging manifest escaped candidate install root: $manifestPath -> $hostPath"
        }
        [ordered]@{
            registry = $_
            manifest = [IO.Path]::GetFullPath($manifestPath)
            manifest_sha256 = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
            host = $hostPath
            host_sha256 = (Get-FileHash -LiteralPath $hostPath -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    })
}

$manifestPath = (Resolve-Path -LiteralPath $CandidateManifestPath).Path
$manifestDir = Split-Path $manifestPath -Parent
$manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
$currentCommit = (& git -C $repo rev-parse HEAD).Trim()
$currentTree = (& git -C $repo rev-parse 'HEAD^{tree}').Trim()
if ([int]$manifest.schema -ne 1 -or [string]$manifest.package_tier -ne 'candidate' -or [string]$manifest.source_commit -ne $currentCommit -or [string]$manifest.source_tree -ne $currentTree) {
    throw 'Browser media-push gate requires a candidate from the current source commit/tree.'
}
$candidateMsi = Resolve-FullPath ([string]$manifest.artifacts.msi.path) $manifestDir
if (-not (Test-Path -LiteralPath $candidateMsi -PathType Leaf)) { throw "Candidate MSI is missing: $candidateMsi" }
$candidateMsiHash = (Get-FileHash -LiteralPath $candidateMsi -Algorithm SHA256).Hash.ToLowerInvariant()
if ($candidateMsiHash -ne ([string]$manifest.artifacts.msi.sha256).ToLowerInvariant()) { throw 'Candidate MSI hash does not match the candidate manifest.' }
$productVersion = [string]$manifest.product_version
$installer = New-Object -ComObject WindowsInstaller.Installer
try {
    $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($candidateMsi, 0))
    try {
        $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @("SELECT ``Value`` FROM ``Property`` WHERE ``Property``='ProductCode'"))
        try {
            $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
            $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
            try { $candidateProductCode = [string]$record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, 1) }
            finally { if ($null -ne $record) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) | Out-Null } }
        } finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) | Out-Null }
    } finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) | Out-Null }
} finally { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null }

$related = Get-InstalledProduct '{1C80D5F7-A1EC-4BAE-A4A6-E010C5A3EE6B}'
if ($null -ne $related) { throw "Browser media-push gate requires a clean install state; related product is already installed: $($related.product_code) $($related.version)" }

$python = if ($Python) { (Resolve-Path -LiteralPath $Python).Path } else { (Get-Command python.exe -ErrorAction Stop).Source }
$ffmpeg = if ($Ffmpeg) { (Resolve-Path -LiteralPath $Ffmpeg).Path } elseif ($env:HLS_V7_FFMPEG_DIR) { (Resolve-Path -LiteralPath (Join-Path $env:HLS_V7_FFMPEG_DIR 'ffmpeg.exe')).Path } else { (Get-Command ffmpeg.exe -ErrorAction Stop).Source }
$edge = Resolve-Browser $EdgeBinary @((Join-Path ${env:ProgramFiles(x86)} 'Microsoft\Edge\Application\msedge.exe'), (Join-Path $env:ProgramFiles 'Microsoft\Edge\Application\msedge.exe')) 'Edge'
$firefox = Resolve-Browser $FirefoxBinary @((Join-Path $env:ProgramFiles 'Mozilla Firefox\firefox.exe'), (Join-Path ${env:ProgramFiles(x86)} 'Mozilla Firefox\firefox.exe')) 'Firefox'
$edgeDriverResolved = if ($EdgeDriver) { (Resolve-Path -LiteralPath $EdgeDriver).Path } else { '' }
$firefoxDriverResolved = if ($FirefoxDriver) { (Resolve-Path -LiteralPath $FirefoxDriver).Path } else { '' }

$evidenceRoot = Join-Path $repo 'artifacts\v7-productization\release-evidence\browser-media-push'
$installLog = Join-Path $evidenceRoot 'candidate-install.log'
$uninstallLog = Join-Path $evidenceRoot 'candidate-uninstall.log'
Remove-Item -LiteralPath $evidenceRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $evidenceRoot | Out-Null
$previousData = $env:HLS_V7_DATA_DIR
$previousDownloads = $env:HLS_V7_DOWNLOAD_DIR
$previousCoreTcp = $env:HLS_V7_CORE_TCP
$previousCoreBind = $env:HLS_V7_CORE_BIND
$previousSkip = $env:HLS_V6_SKIP_MIGRATE
$installed = $false
$application = $null
$engine = $null
try {
    $exit = Invoke-Msi @('/i', $candidateMsi, '/qn', "INSTALLDIR=$InstallDir") $installLog
    if ($exit -notin @(0, 3010, 1641)) { throw "Candidate MSI installation failed with exit $exit" }
    $installed = $true
    $installedProduct = Get-InstalledProduct '{1C80D5F7-A1EC-4BAE-A4A6-E010C5A3EE6B}'
    if ($null -eq $installedProduct -or [string]$installedProduct.version -ne $productVersion) { throw 'Candidate MSI did not register the expected product version.' }
    $hostName = 'com.ciaooo55.hls_downloader'
    $edgeRegistration = @(Get-NativeRegistration $InstallDir $hostName | Where-Object { $_.registry -like '*\Microsoft\Edge\*' })
    $firefoxRegistration = @(Get-NativeRegistration $InstallDir $hostName | Where-Object { $_.registry -like '*\Mozilla\*' })
    if ($edgeRegistration.Count -ne 1) { throw "Candidate MSI did not install exactly one Edge Native Messaging registration: $($edgeRegistration | ConvertTo-Json -Depth 5 -Compress)" }
    if ($firefoxRegistration.Count -ne 1) { throw "Candidate MSI did not install exactly one Firefox Native Messaging registration: $($firefoxRegistration | ConvertTo-Json -Depth 5 -Compress)" }

    $extensionRoot = Join-Path $evidenceRoot 'extensions'
    New-Item -ItemType Directory -Force -Path $extensionRoot | Out-Null
    $extensionPaths = @{}
    foreach ($name in @('Chromium','Firefox')) {
        $entry = $manifest.extensions.$name
        if ($null -eq $entry -or [String]::IsNullOrWhiteSpace([string]$entry.path)) { throw "Candidate manifest is missing $name extension entry." }
        $archive = Resolve-FullPath ([string]$entry.path) $manifestDir
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne ([string]$entry.sha256).ToLowerInvariant()) { throw "$name extension hash does not match candidate manifest." }
        $destination = Join-Path $extensionRoot $name.ToLowerInvariant()
        Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force
        $extensionPaths[$name] = $destination
    }

    $dataRoot = Join-Path $evidenceRoot 'runtime-data'
    $downloadRoot = Join-Path $evidenceRoot 'runtime-downloads'
    $port = Get-FreePort
    $env:HLS_V7_DATA_DIR = $dataRoot
    $env:HLS_V7_DOWNLOAD_DIR = $downloadRoot
    $env:HLS_V7_CORE_TCP = '1'
    $env:HLS_V7_CORE_BIND = "127.0.0.1:$port"
    $env:HLS_V6_SKIP_MIGRATE = '1'
    $launcher = Join-Path $InstallDir 'HLSDownloader.exe'
    if (-not (Test-Path -LiteralPath $launcher -PathType Leaf)) { throw "Installed candidate launcher is missing: $launcher" }
    $application = Start-Process -FilePath $launcher -WorkingDirectory $InstallDir -PassThru
    $deadline = (Get-Date).AddSeconds(45)
    do {
        Start-Sleep -Milliseconds 250
        $engine = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
            $_.Name -eq 'HLSDownloaderEngine.exe' -and $_.ExecutablePath -and $_.ExecutablePath.StartsWith($InstallDir, [StringComparison]::OrdinalIgnoreCase)
        }) | Select-Object -First 1
    } while (-not $engine -and (Get-Date) -lt $deadline)
    if (-not $engine) { throw 'Installed candidate did not start its Core engine.' }

    $accessBridgeDll = Get-ChildItem -LiteralPath $InstallDir -Filter 'WindowsAccessBridge-64.dll' -File -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $accessBridgeDll) { throw 'Installed candidate does not contain WindowsAccessBridge-64.dll.' }
    $browserReports = @{}
    foreach ($browser in @('edge','firefox')) {
        $report = Join-Path $evidenceRoot "$browser-tvbox-real.json"
        $args = @(
            (Join-Path $repo 'scripts\smoke_extension_tvbox_real.py'),
            '--browser', $browser,
            '--extension', [string]$extensionPaths[$(if ($browser -eq 'edge') { 'Chromium' } else { 'Firefox' })],
            '--core-port', [string]$port,
            '--expected-host', $ExpectedTvboxHost,
            '--ffmpeg', $ffmpeg,
            '--access-bridge-python', $python,
            '--access-bridge-dll', $accessBridgeDll.FullName,
            '--output', $report
        )
        if ($browser -eq 'edge') {
            $args += @('--browser-binary', $edge)
            if ($edgeDriverResolved) { $args += @('--driver', $edgeDriverResolved) }
        } else {
            $args += @('--browser-binary', $firefox)
            if ($firefoxDriverResolved) { $args += @('--driver', $firefoxDriverResolved) }
        }
        & $python @args | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "$browser browser-to-TVBox smoke failed with exit $LASTEXITCODE" }
        $parsed = Get-Content -LiteralPath $report -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($parsed.passed -ne $true -or [string]$parsed.browser -ne $browser -or [string]$parsed.expected_receiver_host -ne $ExpectedTvboxHost) {
            throw "$browser media-push report identity mismatch."
        }
        $browserReports[$browser] = [ordered]@{
            path = $report
            sha256 = (Get-FileHash -LiteralPath $report -Algorithm SHA256).Hash.ToLowerInvariant()
            browser_identity = $parsed.browser_identity
            selected_device = $parsed.selected_device
            receiver_requests = @($parsed.receiver_requests)
        }
    }

    $summary = [ordered]@{
        schema = 1
        passed = $true
        product_version = $productVersion
        source_commit = $currentCommit
        source_tree = $currentTree
        candidate_manifest_sha256 = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant()
        candidate_msi_sha256 = $candidateMsiHash
        install_dir = $InstallDir
        expected_receiver_host = $ExpectedTvboxHost
        edge_registration = $edgeRegistration
        firefox_registration = $firefoxRegistration
        browsers = $browserReports
    }
    Write-Output ($summary | ConvertTo-Json -Depth 10 -Compress)
} finally {
    if ($application -and -not $application.HasExited) { Stop-Process -Id $application.Id -Force -ErrorAction SilentlyContinue }
    @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($InstallDir, [StringComparison]::OrdinalIgnoreCase) }) | ForEach-Object {
        Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue
    }
    if ($installed) {
        $uninstallExit = Invoke-Msi @('/x', $candidateProductCode, '/qn') $uninstallLog
        if ($uninstallExit -notin @(0, 3010, 1641)) { Write-Warning "Candidate uninstall returned $uninstallExit" }
    }
    $env:HLS_V7_DATA_DIR = $previousData
    $env:HLS_V7_DOWNLOAD_DIR = $previousDownloads
    $env:HLS_V7_CORE_TCP = $previousCoreTcp
    $env:HLS_V7_CORE_BIND = $previousCoreBind
    $env:HLS_V6_SKIP_MIGRATE = $previousSkip
}
