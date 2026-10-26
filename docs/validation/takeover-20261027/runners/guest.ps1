$ErrorActionPreference='Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
$env:HLS_VM_ACCEPTANCE='1'
$env:PYTHONDONTWRITEBYTECODE='1'
$env:TEMP='C:\QAWork\tmp'
$env:TMP=$env:TEMP
New-Item -ItemType Directory -Force C:\QAWork,$env:TEMP | Out-Null
Set-Location C:\QAWork
$result=[ordered]@{passed=$false;finished=$false;current='setup';checks=@()}
function Save { $result | ConvertTo-Json -Depth 8 | Set-Content C:\QAReports\summary.json -Encoding UTF8 }
function Run([string]$exe,[string[]]$arguments,[string]$name) {
    $result.current=$name;Save
    $p=Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "C:\QAReports\$name.stdout.txt" -RedirectStandardError "C:\QAReports\$name.stderr.txt"
    [void]$p.Handle
    if (-not $p.WaitForExit(600000)) { Stop-Process -Id $p.Id -Force; throw "$name timeout" }
    if ($p.ExitCode -ne 0) { throw "$name failed: $($p.ExitCode)" }
    $result.checks+=@{name=$name;passed=$true};Save
}
try {
    Save
    Copy-Item C:\QAPerf\runtime C:\QAWork\runtime -Recurse
    New-Item -ItemType File C:\QAWork\runtime\portable -Force | Out-Null
    Copy-Item C:\QAPerf\python C:\QAWork\python -Recurse
    "python312.zip`n.`nC:\QAInput\pydeps`nC:\QAPerf\pydeps`nC:\QAScripts" | Set-Content C:\QAWork\python\python312._pth -Encoding ASCII
    Copy-Item C:\QAInput\firefox-setup.exe C:\QAWork\firefox-setup.exe
    Run C:\QAWork\firefox-setup.exe @('/S','/InstallDirectoryPath=C:\QAFirefox') 'firefox-install'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $edge='C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
    $version=(Get-Item $edge).VersionInfo.ProductVersion
    Invoke-WebRequest -UseBasicParsing "https://msedgedriver.microsoft.com/$version/edgedriver_win64.zip" -OutFile C:\QAWork\edge-driver.zip
    [IO.Compression.ZipFile]::ExtractToDirectory('C:\QAWork\edge-driver.zip','C:\QAWork\edge-driver')
    Start-Process C:\QAWork\runtime\app\resources\HLSDownloaderEngine.exe -WorkingDirectory C:\QAWork\runtime -WindowStyle Hidden | Out-Null
    foreach ($family in @('firefox','edge')) {
        $binary=if ($family -eq 'firefox') {'C:\QAFirefox\firefox.exe'} else {'"'+$edge+'"'}
        $driver=if ($family -eq 'firefox') {'C:\QAInput\gecko\geckodriver.exe'} else {'C:\QAWork\edge-driver\msedgedriver.exe'}
        $extension=if ($family -eq 'firefox') {'C:\QAExtensions\firefox-mv3'} else {'C:\QAExtensions\chrome-mv3'}
        Run C:\QAWork\python\python.exe @('C:\QAScripts\smoke_completed_browser_download.py','--browser',$family,'--runtime','C:\QAWork\runtime','--extension',$extension,'--binary',$binary,'--driver',$driver,'--report',"C:\QAReports\$family-completed.json") "$family-completed"
    }
    Run C:\QAWork\python\python.exe @('C:\QAScripts\smoke_firefox_response_takeover.py','--browser','firefox','--runtime','C:\QAWork\runtime','--extension','C:\QAExtensions\firefox-mv3','--binary','C:\QAFirefox\firefox.exe','--driver','C:\QAInput\gecko\geckodriver.exe','--report','C:\QAReports\firefox-response.json','--public') 'firefox-response'
    $result.current='await-package';Save
    $deadline=[DateTime]::UtcNow.AddMinutes(30)
    while (-not (Test-Path C:\QAInput\package.ready)) {
        if ([DateTime]::UtcNow -gt $deadline) { throw 'Final package not supplied' }
        Start-Sleep -Seconds 2
    }
    Run powershell.exe @('-NoProfile','-ExecutionPolicy','Bypass','-File','C:\QARunners\installed.ps1') 'installed'
    $result.passed=$true
} catch {$result.error=$_.Exception.Message;$result.position=$_.InvocationInfo.PositionMessage;$result.stack=$_.ScriptStackTrace}
finally {$result.finished=$true;Save}
