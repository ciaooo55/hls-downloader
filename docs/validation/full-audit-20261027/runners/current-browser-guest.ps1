param([switch]$OfflineOnly, [switch]$VerifyOfflineAfterMatrix, [switch]$Firefox, [switch]$DependenciesOnly)
$ErrorActionPreference = 'Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
$report = 'C:\QACurrentReports'
$work = 'C:\QACurrentWork'
$browserReport = if ($Firefox) { 'firefox.json' } else { 'edge.json' }
$result = [ordered]@{ passed = $false; finished = $false; phase = 'setup'; host_install_performed = $false }
function Save-Result { $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath "$report\bootstrap.json" -Encoding UTF8 }
try {
    New-Item -ItemType Directory -Force $work, "$work\product\app\resources", "$work\temp" | Out-Null
    $env:TEMP = "$work\temp"
    $env:TMP = $env:TEMP
    $env:HLS_VM_ACCEPTANCE = '1'
    $env:HLS_V7_PIPE = '\\.\pipe\HLSDownloader.current-browser-20261029'
    $env:SLINT_BACKEND = 'winit-software'
    $resources = "$work\product\app\resources"
    if (-not $OfflineOnly) {
    $result.phase = 'extract-unchanged-presenter'; Save-Result
    # Windows PowerShell 5.1 reads BOM-less scripts using the ANSI code page.
    # Discover the exact dated MSI so its non-ASCII suffix is never a source literal.
    $installers = @(Get-ChildItem -LiteralPath 'C:\QAExistingDelivery' -Filter 'HLSDownloader-7.0.3-20261028-*.msi' -File)
    if ($installers.Count -ne 1) { throw "Expected one dated MSI, found $($installers.Count)" }
    $msi = $installers[0].FullName
    $result.presenter_source_msi_sha256 = (Get-FileHash -LiteralPath $msi).Hash.ToLowerInvariant()
    if ($result.presenter_source_msi_sha256 -ne '8945ab49b5a77dac24f689cd80565184956345f2c8d8cc52c1b58426c7b70bd8') { throw 'Packaged presenter reference MSI mismatch' }
    $extract = Start-Process msiexec.exe -ArgumentList @('/a', "`"$msi`"", '/qn', 'TARGETDIR=C:\QACurrentWork\extracted', '/l*v', 'C:\QACurrentReports\extraction.log') -WindowStyle Hidden -PassThru
    [void]$extract.Handle
    if (-not $extract.WaitForExit(120000)) { Stop-Process -Id $extract.Id -Force; throw 'MSI extraction timed out' }
    if ($extract.ExitCode -ne 0) { throw "MSI extraction failed: $($extract.ExitCode)" }
    $presenter = @(Get-ChildItem -LiteralPath "$work\extracted" -Filter HLSDownloaderPresenter.exe -File -Recurse)
    if ($presenter.Count -ne 1) { throw "Expected one real presenter, found $($presenter.Count)" }
    Copy-Item -LiteralPath $presenter[0].FullName -Destination "$resources\HLSDownloaderPresenter.exe"
    if ($DependenciesOnly) {
        if (-not (Test-Path -LiteralPath 'C:\QADependencyCache' -PathType Container)) { throw 'Dependency cache mapping required' }
        $result.dependencies = @()
        foreach ($name in @('libmpv-2.dll', 'ffmpeg.exe')) {
            $found = @(Get-ChildItem -LiteralPath "$work\extracted" -Filter $name -File -Recurse)
            if ($found.Count -ne 1) { throw "Expected one packaged dependency: $name" }
            $target = Join-Path 'C:\QADependencyCache' $name
            Copy-Item -LiteralPath $found[0].FullName -Destination $target
            $result.dependencies += @{ name = $name; sha256 = (Get-FileHash -LiteralPath $target).Hash.ToLowerInvariant() }
        }
        $result.passed = $true
        $result.phase = 'verified-r22-media-dependencies-extracted'
        return
    }
    } else {
        $result.presenter_not_used_for_absent_host_case = $true
    }
    Copy-Item -LiteralPath C:\QACurrentBinaries\hls-downloader-engine.exe -Destination "$resources\HLSDownloaderEngine.exe"
    Copy-Item -LiteralPath C:\QACurrentBinaries\HLSDownloaderNativeHost.exe -Destination "$resources\HLSDownloaderNativeHost.exe"
    Copy-Item -LiteralPath C:\QACurrentInput\vcruntime140.dll -Destination "$resources\vcruntime140.dll"
    New-Item -ItemType File -Path "$work\product\portable" | Out-Null
    $result.components = @(Get-ChildItem -LiteralPath $resources -File | ForEach-Object { @{ name = $_.Name; sha256 = (Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant() } })
    $env:HLS_REPLY_REAL_HOST = "$resources\HLSDownloaderNativeHost.exe"
    $env:HLS_REPLY_PROXY_STATE = "$report\proxy"
    $env:HLS_REPLY_FAILURE_COUNT = '1'
    $env:HLS_REPLY_PROXY_PATH = "$work\NativeReplyProxy.exe"
    if (-not $OfflineOnly) {
        New-Item -ItemType Directory -Force $env:HLS_REPLY_PROXY_STATE | Out-Null
        Add-Type -Path C:\QACurrentRunners\NativeReplyProxy.cs -ReferencedAssemblies System.Web.Extensions,System.Core -OutputAssembly $env:HLS_REPLY_PROXY_PATH -OutputType ConsoleApplication
    }
    $result.phase = 'real-browser-current-source'; Save-Result
    $result.runners = @(Get-Item C:\QACurrentRunners\current-browser-guest.ps1, C:\QACurrentRunners\current-browser-guest.mjs, C:\QACurrentRunners\firefox-marionette.mjs, C:\QACurrentRunners\NativeReplyProxy.cs | ForEach-Object { @{ name = $_.Name; sha256 = (Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant() } })
    $probeArguments = @('C:\QACurrentRunners\current-browser-guest.mjs')
    if ($Firefox) { $probeArguments += '--firefox' }
    if ($OfflineOnly) { $probeArguments += '--offline-only' }
    $probe = Start-Process C:\QACurrentInput\node.exe -ArgumentList $probeArguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "$report\probe.stdout.txt" -RedirectStandardError "$report\probe.stderr.txt"
    [void]$probe.Handle
    if (-not $probe.WaitForExit(240000)) { Stop-Process -Id $probe.Id -Force; throw 'Browser probe timed out' }
    if ($probe.ExitCode -ne 0) { throw "Browser probe failed: $($probe.ExitCode)" }
    $result.passed = (Get-Content -LiteralPath "$report\$browserReport" -Raw -Encoding UTF8 | ConvertFrom-Json).passed
    if ($VerifyOfflineAfterMatrix -and $result.passed) {
        $result.phase = 'initial-offline-after-fix'; Save-Result
        $env:HLS_CURRENT_REPORT_DIR = "$report\initial-offline"
        New-Item -ItemType Directory -Force $env:HLS_CURRENT_REPORT_DIR | Out-Null
        $offlineArguments = @('C:\QACurrentRunners\current-browser-guest.mjs', '--offline-only')
        if ($Firefox) { $offlineArguments += '--firefox' }
        $offline = Start-Process C:\QACurrentInput\node.exe -ArgumentList $offlineArguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "$report\initial-offline\probe.stdout.txt" -RedirectStandardError "$report\initial-offline\probe.stderr.txt"
        [void]$offline.Handle
        if (-not $offline.WaitForExit(90000)) { Stop-Process -Id $offline.Id -Force; throw 'Initial offline verification timed out' }
        if ($offline.ExitCode -ne 0) { throw "Initial offline verification failed: $($offline.ExitCode)" }
        $result.passed = (Get-Content -LiteralPath "$report\initial-offline\$browserReport" -Raw -Encoding UTF8 | ConvertFrom-Json).passed
    }
} catch {
    $result.passed = $false
    $result.error = $_.Exception.Message
    $result.position = $_.InvocationInfo.PositionMessage
} finally {
    Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith("$work\", [StringComparison]::OrdinalIgnoreCase) } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    $result.finished = $true
    Save-Result
}
