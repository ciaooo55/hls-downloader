$ErrorActionPreference='Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
Add-Type -Namespace QA -Name MSI -MemberDefinition '[System.Runtime.InteropServices.DllImport("msi.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode)] public static extern int MsiQueryProductState(string product);'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$meta=Get-Content C:\QAInput\delivery\installer.json -Raw | ConvertFrom-Json
$old=Get-Content C:\QAPerf\input\final-manifest.json -Raw | ConvertFrom-Json
$result=[ordered]@{passed=$false;checks=@()}
function Save { $result | ConvertTo-Json -Depth 8 | Set-Content C:\QAReports\installer.json -Encoding UTF8 }
function Record($name,$actual) { $result.checks+=@{name=$name;actual=$actual};Save }
function Run([string]$exe,[string[]]$arguments,[string]$name) {
    $p=Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "C:\QAReports\$name.stdout.txt" -RedirectStandardError "C:\QAReports\$name.stderr.txt"
    [void]$p.Handle
    if (-not $p.WaitForExit(600000)) { Stop-Process -Id $p.Id -Force;throw "$name timeout" }
    if ($p.ExitCode -ne 0) { throw "$name failed: $($p.ExitCode)" }
}
$installed=$false
try {
    Get-CimInstance Win32_Process | Where-Object {$_.ExecutablePath -and $_.ExecutablePath.StartsWith('C:\QAWork\runtime\',[StringComparison]::OrdinalIgnoreCase)} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Copy-Item C:\QAPerf\input\final.msi C:\QAWork\baseline.msi
    Copy-Item C:\QAInput\delivery\candidate.msi C:\QAWork\candidate.msi
    if ((Get-FileHash C:\QAWork\candidate.msi).Hash.ToLowerInvariant() -ne $meta.sha256) { throw 'MSI identity mismatch' }
    Run msiexec.exe @('/i','C:\QAWork\baseline.msi','/qn','INSTALLDIR=C:\QAUpgrade','/l*v','C:\QAReports\baseline-install.log') 'baseline-install'
    $installed=$true
    Start-Process C:\QAUpgrade\app\resources\HLSDownloaderEngine.exe -WorkingDirectory C:\QAUpgrade -WindowStyle Hidden | Out-Null
    Run msiexec.exe @('/i','C:\QAWork\candidate.msi','/qn','/l*v','C:\QAReports\candidate-upgrade.log') 'candidate-upgrade'
    if ([QA.MSI]::MsiQueryProductState($old.product_code) -ne -1 -or [QA.MSI]::MsiQueryProductState($meta.product_code) -ne 5) { throw 'Upgrade registration mismatch' }
    Record 'upgrade while previous Core runs, old MSI removed' $true
    $components=Get-Content C:\QAInput\delivery\components.json -Raw | ConvertFrom-Json
    foreach ($property in $components.PSObject.Properties) {
        $path=Join-Path C:\QAUpgrade $property.Name
        if ((Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -ne $property.Value) { throw "Installed component mismatch: $($property.Name)" }
    }
    Record 'all installed components match candidate' @($components.PSObject.Properties).Count
    foreach ($family in @('Chromium','Firefox')) {
        [IO.Compression.ZipFile]::ExtractToDirectory("C:\QAUpgrade\app\resources\extensions\HLSDownloader-7.0.3-$family.zip","C:\QAWork\Installed$family")
    }
    Start-Process C:\QAUpgrade\app\resources\HLSDownloaderEngine.exe -WorkingDirectory C:\QAUpgrade -WindowStyle Hidden | Out-Null
    foreach ($family in @('firefox','edge')) {
        $binary=if ($family -eq 'firefox') {'C:\QAFirefox\firefox.exe'} else {'"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"'}
        $driver=if ($family -eq 'firefox') {'C:\QAInput\gecko\geckodriver.exe'} else {'C:\QAWork\edge-driver\msedgedriver.exe'}
        $extension=if ($family -eq 'firefox') {'C:\QAWork\InstalledFirefox'} else {'C:\QAWork\InstalledChromium'}
        Run C:\QAWork\python\python.exe @('C:\QAScripts\smoke_completed_browser_download.py','--browser',$family,'--runtime','C:\QAUpgrade','--extension',$extension,'--binary',$binary,'--driver',$driver,'--report',"C:\QAReports\installed-$family-completed.json") "installed-$family-completed"
        Record "installed $family completed-download ownership" $true
    }
    Run C:\QAWork\python\python.exe @('C:\QAScripts\smoke_firefox_response_takeover.py','--browser','firefox','--runtime','C:\QAUpgrade','--extension','C:\QAWork\InstalledFirefox','--binary','C:\QAFirefox\firefox.exe','--driver','C:\QAInput\gecko\geckodriver.exe','--report','C:\QAReports\installed-firefox-response.json','--public') 'installed-firefox-response'
    Record 'installed Firefox response blocking and public EXE' $true
    Run msiexec.exe @('/x',$meta.product_code,'/qn','/l*v','C:\QAReports\candidate-uninstall.log') 'candidate-uninstall'
    $installed=$false
    if ([QA.MSI]::MsiQueryProductState($meta.product_code) -ne -1 -or (Test-Path C:\QAUpgrade\HLSDownloader.exe)) { throw 'Uninstall incomplete' }
    Record 'upgrade candidate uninstalled' $true
    Run C:\QAWork\python\python.exe @('C:\QAScripts\smoke_v7_installed_footprint.py','--msi','C:\QAInput\delivery\candidate.msi','--manifest','C:\QAInput\delivery\installer.json','--expected-components','C:\QAInput\delivery\components.json','--verify-native-host-startup','--report','C:\QAReports\fresh-installed.json') 'fresh-installed'
    Record 'fresh custom path, download, Native Host and uninstall' $true
    $result.passed=$true
} catch {$result.error=$_.Exception.Message;$result.position=$_.InvocationInfo.PositionMessage;throw}
finally {
    if ($installed) { Start-Process msiexec.exe -ArgumentList @('/x',$meta.product_code,'/qn') -WindowStyle Hidden -Wait }
    Save
}
