$ErrorActionPreference = 'Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
$env:HLS_VM_ACCEPTANCE = '1'
$env:PYTHONPATH = 'C:\QAInput\pydeps'
$env:PYTHONDONTWRITEBYTECODE = '1'
$env:TEMP = 'C:\QAWork\tmp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force C:\QAWork, $env:TEMP | Out-Null
$result = [ordered]@{passed=$false; finished=$false; current='setup'; checks=@(); error=''}
function Save { $result | ConvertTo-Json -Depth 10 | Set-Content C:\QAReports\summary.json -Encoding UTF8 }
function Record($name, $actual) { $result.checks += [ordered]@{name=$name;actual=$actual}; Save }
function Run($exe, [string[]]$arguments, $name) {
    $result.current=$name; Save
    $p=Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "C:\QAReports\$name.stdout.txt" -RedirectStandardError "C:\QAReports\$name.stderr.txt"
    [void]$p.Handle
    if (-not $p.WaitForExit(600000)) { Stop-Process -Id $p.Id -Force; throw "$name timeout" }
    if ($p.ExitCode -ne 0) { throw "$name exited $($p.ExitCode)" }
}
$metadata = Get-Content C:\QAInput\candidate\installer.json -Raw | ConvertFrom-Json
Add-Type -Namespace QA -Name MSI -MemberDefinition '[System.Runtime.InteropServices.DllImport("msi.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)] public static extern int MsiQueryProductState(string product);'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$installed=$false
try {
    Save
    Copy-Item C:\QAInput\input\baseline.msi C:\QAWork\baseline.msi
    Copy-Item C:\QAInput\input\candidate.msi C:\QAWork\candidate.msi
    if ((Get-FileHash C:\QAWork\candidate.msi).Hash.ToLowerInvariant() -ne $metadata.sha256) { throw 'MSI hash mismatch' }
    Run msiexec.exe @('/i','C:\QAWork\baseline.msi','/qn','INSTALLDIR=C:\QAInstalled','/l*v','C:\QAReports\baseline-install.log') 'baseline-install'
    $installed=$true
    Start-Process C:\QAInstalled\app\resources\HLSDownloaderEngine.exe -WorkingDirectory C:\QAInstalled -WindowStyle Hidden | Out-Null
    Run msiexec.exe @('/i','C:\QAWork\candidate.msi','/qn','/l*v','C:\QAReports\candidate-upgrade.log') 'candidate-upgrade'
    if ([QA.MSI]::MsiQueryProductState('{83EEE747-04FF-47E9-B973-E934B32C5F70}') -ne -1) { throw 'Old MSI still registered' }
    if ([QA.MSI]::MsiQueryProductState($metadata.product_code) -ne 5) { throw 'New MSI missing' }
    Record 'running-Core upgrade' $metadata
    foreach($family in @('Firefox','Chromium')) {
        $zip="C:\QAInstalled\app\resources\extensions\HLSDownloader-7.0.3-$family.zip"
        $expected=(Get-FileHash "C:\QAInput\input\extensions\$family.zip").Hash
        if ((Get-FileHash $zip).Hash -ne $expected) { throw "$family ZIP hash mismatch" }
        [IO.Compression.ZipFile]::ExtractToDirectory($zip, "C:\QAWork\$family")
        Record "$family installed extension identity" $expected
    }
    if ((Get-FileHash C:\QAInstalled\app\resources\BUILD-PROVENANCE.json).Hash -ne (Get-FileHash C:\QAInput\candidate\BUILD-PROVENANCE.json).Hash) { throw 'Provenance mismatch' }
    Start-Process C:\QAInstalled\app\resources\HLSDownloaderEngine.exe -WorkingDirectory C:\QAInstalled -WindowStyle Hidden | Out-Null
    Copy-Item C:\QAInput\input\firefox-setup.exe C:\QAWork\firefox-setup.exe
    Run C:\QAWork\firefox-setup.exe @('/S','/InstallDirectoryPath=C:\QAFirefox') 'firefox-setup'
    $edge='C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
    $edgeVersion=(Get-Item $edge).VersionInfo.ProductVersion
    Invoke-WebRequest -UseBasicParsing "https://msedgedriver.microsoft.com/$edgeVersion/edgedriver_win64.zip" -OutFile C:\QAWork\edge-driver.zip
    [IO.Compression.ZipFile]::ExtractToDirectory('C:\QAWork\edge-driver.zip', 'C:\QAWork\edge-driver')
    Record 'browser versions' @{edge=$edgeVersion;firefox=(Get-Item C:\QAFirefox\firefox.exe).VersionInfo.ProductVersion;edge_driver_sha256=(Get-FileHash C:\QAWork\edge-driver\msedgedriver.exe).Hash}
    $script='C:\QASource\scripts\validation\smoke_firefox_response_takeover.py'
    Run C:\QAPython\python.exe @($script,'--runtime','C:\QAInstalled','--extension','C:\QAWork\Firefox','--binary','C:\QAFirefox\firefox.exe','--driver','C:\QAInput\input\gecko\geckodriver.exe','--report','C:\QAReports\firefox.json','--public') 'firefox-browser'
    Run C:\QAPython\python.exe @($script,'--browser','edge','--runtime','C:\QAInstalled','--extension','C:\QAWork\Chromium','--binary',('"'+$edge+'"'),'--driver','C:\QAWork\edge-driver\msedgedriver.exe','--report','C:\QAReports\edge.json','--public') 'edge-browser'
    foreach($family in @('firefox','edge')) {
        $report=Get-Content "C:\QAReports\$family.json" -Raw | ConvertFrom-Json
        if ($report.passed -ne $true) { throw "$family acceptance failed" }
        Record "$family real-browser acceptance" $true
    }
    $result.passed=$true
} catch { $result.error=$_.Exception.Message; $result.position=$_.InvocationInfo.PositionMessage; $result.stack=$_.ScriptStackTrace }
finally {
    if ($installed) {
        try {
            Run msiexec.exe @('/x',$metadata.product_code,'/qn','/l*v','C:\QAReports\candidate-uninstall.log') 'candidate-uninstall'
            if ([QA.MSI]::MsiQueryProductState($metadata.product_code) -ne -1 -or (Test-Path C:\QAInstalled\app\resources\HLSDownloaderEngine.exe)) { throw 'Uninstall incomplete' }
            Record 'candidate uninstalled' $true
        } catch { $result.passed=$false; $result.error += '; '+$_.Exception.Message }
    }
    $result.finished=$true; Save
}
