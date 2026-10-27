$ErrorActionPreference='Stop'
if ($env:USERNAME -ne 'WDAGUtilityAccount') { throw 'Windows Sandbox only' }
$env:HLS_VM_ACCEPTANCE='1'
$env:PYTHONDONTWRITEBYTECODE='1'
$env:TEMP='C:\QAWork\tmp'
$env:TMP=$env:TEMP
New-Item -ItemType Directory -Force C:\QAWork,$env:TEMP | Out-Null
Set-Location C:\QAWork
$completed=@{}
'ready' | Set-Content C:\QAReports\jobs-ready.txt
while (-not (Test-Path C:\QAInput\stop-jobs)) {
    foreach ($ready in Get-ChildItem C:\QAInput\jobs -Filter '*.ready' -ErrorAction SilentlyContinue | Sort-Object Name) {
        if ($completed.ContainsKey($ready.BaseName)) { continue }
        $job=Join-Path $ready.DirectoryName ($ready.BaseName+'.ps1')
        $result=[ordered]@{name=$ready.BaseName;finished=$false;exit_code=$null;started=[DateTime]::UtcNow.ToString('o')}
        $path='C:\QAReports\job-'+$ready.BaseName+'.json'
        $result | ConvertTo-Json | Set-Content $path -Encoding UTF8
        $p=Start-Process powershell.exe -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File',$job) -WindowStyle Hidden -PassThru -RedirectStandardOutput ('C:\QAReports\job-'+$ready.BaseName+'.stdout.txt') -RedirectStandardError ('C:\QAReports\job-'+$ready.BaseName+'.stderr.txt')
        [void]$p.Handle
        if (-not $p.WaitForExit(1800000)) { Stop-Process -Id $p.Id -Force; $result.exit_code=-1 } else { $result.exit_code=$p.ExitCode }
        $result.finished=$true
        $result | ConvertTo-Json | Set-Content $path -Encoding UTF8
        $completed[$ready.BaseName]=$true
    }
    Start-Sleep -Seconds 1
}
