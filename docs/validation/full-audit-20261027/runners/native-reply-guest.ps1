param([ValidateSet('edge','firefox')][string[]]$Browsers=@('edge','firefox'))
$ErrorActionPreference='Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
$env:HLS_VM_ACCEPTANCE='1'
$env:PYTHONDONTWRITEBYTECODE='1'
$env:TEMP='C:\QAWork\tmp'
$env:TMP=$env:TEMP
New-Item -ItemType Directory -Force C:\QAWork,$env:TEMP | Out-Null
Set-Location C:\QAWork
$result=[ordered]@{passed=$false;finished=$false;phase='setup';host_install_performed=$false;browsers=$Browsers;checks=@()}
function Save { $result | ConvertTo-Json -Depth 8 | Set-Content C:\QAReports\summary.json -Encoding UTF8 }
function Run([string]$exe,[string[]]$arguments,[string]$name,[int]$timeout=180000) {
    $result.phase=$name;Save
    $quoted=@($arguments | ForEach-Object {
        if($_ -match '[\s"]') { '"' + ($_ -replace '(\\*)"','$1$1\"' -replace '(\\+)$','$1$1') + '"' } else { $_ }
    })
    $result.invocation=@{executable=$exe;arguments=$quoted};Save
    $process=Start-Process -FilePath $exe -ArgumentList $quoted -WindowStyle Hidden -PassThru -RedirectStandardOutput "C:\QAReports\$name.stdout.txt" -RedirectStandardError "C:\QAReports\$name.stderr.txt"
    [void]$process.Handle
    if (-not $process.WaitForExit($timeout)) {
        $result.processes=@(Get-Process | Where-Object { $_.Name -match 'msiexec|firefox|setup|NativeReply' } | Select-Object Name,Id,MainWindowTitle)
        Save
        Stop-Process -Id $process.Id -Force
        throw "$name timed out"
    }
    if ($process.ExitCode -ne 0) { throw "$name failed: $($process.ExitCode)" }
    $result.checks+=@{name=$name;passed=$true};Save
}
try {
    Save
    Copy-Item C:\QAPerf\python C:\QAWork\python -Recurse
    "python312.zip`n.`nC:\QAInput\pydeps`nC:\QAPerf\pydeps`nC:\QAScripts" | Set-Content C:\QAWork\python\python312._pth -Encoding ASCII
    $result.phase='stage-current-components';Save
    $manifest=Get-Content C:\QACandidate\components.json -Raw | ConvertFrom-Json
    $meta=Get-Content C:\QACandidate\installer.json -Raw | ConvertFrom-Json
    $result.component_reference_msi_sha256=$meta.sha256
    $result.installer_validation_performed=$false
    $result.components=@()
    New-Item -ItemType Directory -Force C:\QAProduct\app\resources | Out-Null
    New-Item -ItemType File -Path C:\QAProduct\portable | Out-Null
    foreach($pair in @(@('HLSDownloaderNativeHost.exe','HLSDownloaderNativeHost.exe'),@('hls-downloader-engine.exe','HLSDownloaderEngine.exe'),@('hls-downloader-presenter.exe','HLSDownloaderPresenter.exe'))) {
        $source=Join-Path C:\QABinaries $pair[0]
        $target=Join-Path C:\QAProduct\app\resources $pair[1]
        $expected=$manifest.('app/resources/'+$pair[1])
        $actual=(Get-FileHash -LiteralPath $source).Hash.ToLowerInvariant()
        if($actual -ne $expected){throw "Component mismatch: $($pair[1])"}
        Copy-Item -LiteralPath $source -Destination $target
        $result.components+=@{name=$pair[1];sha256=$actual}
    }
    $crt='C:\QAPerf\runtime\app\resources\vcruntime140.dll'
    $crtHash=(Get-FileHash -LiteralPath $crt).Hash.ToLowerInvariant()
    if($crtHash -ne $manifest.'app/resources/vcruntime140.dll'){throw 'CRT component mismatch'}
    Copy-Item -LiteralPath $crt -Destination C:\QAProduct\app\resources\vcruntime140.dll
    $result.components+=@{name='vcruntime140.dll';sha256=$crtHash}
    $env:HLS_REPLY_REAL_HOST='C:\QAProduct\app\resources\HLSDownloaderNativeHost.exe'
    Add-Type -Path C:\QARunners\NativeReplyProxy.cs -ReferencedAssemblies System.Web.Extensions,System.Core -OutputAssembly C:\QAWork\NativeReplyProxy.exe -OutputType ConsoleApplication
    if($Browsers -contains 'firefox') {
        Copy-Item C:\QAInput\firefox-setup.exe C:\QAWork\firefox-setup.exe
        Run C:\QAWork\firefox-setup.exe @('/S','/InstallDirectoryPath=C:\QAFirefox') 'firefox-install' 120000
    }
    $edge='C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
    $version=(Get-Item $edge).VersionInfo.ProductVersion
    $driver='C:\QAFollowup\edge-driver\msedgedriver.exe'
    $driverVersion=(Get-Item $driver).VersionInfo.ProductVersion
    if ($version.Split('.')[0] -ne $driverVersion.Split('.')[0]) { throw "Cached Edge driver mismatch: $version / $driverVersion" }
    foreach ($family in $Browsers) {
        $state="C:\QAWork\$family-state"
        New-Item -ItemType Directory -Force $state | Out-Null
        $env:HLS_REPLY_PROXY_STATE=$state
        $binary=if($family -eq 'firefox'){'C:\QAFirefox\firefox.exe'}else{$edge}
        $browserDriver=if($family -eq 'firefox'){'C:\QAInput\gecko\geckodriver.exe'}else{$driver}
        $extension=if($family -eq 'firefox'){'C:\QAExtensions\firefox-mv3'}else{'C:\QAExtensions\chrome-mv3'}
        Run C:\QAWork\python\python.exe @('C:\QARunners\native-reply-probe.py','--browser',$family,'--extension',$extension,'--binary',$binary,'--driver',$browserDriver,'--proxy','C:\QAWork\NativeReplyProxy.exe','--state',$state,'--report',"C:\QAReports\$family.json") "$family-reply-recovery"
    }
    $result.passed=$true
} catch {$result.error=$_.Exception.Message;$result.position=$_.InvocationInfo.PositionMessage}
finally {
    Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith('C:\QAProduct\',[StringComparison]::OrdinalIgnoreCase) } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    $result.finished=$true;Save
}
