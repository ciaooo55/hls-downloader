$ErrorActionPreference = 'Stop'
$work = 'C:\HLSDeliveryQA'
try {
    New-Item -ItemType Directory -Force C:\HLSTests\scripts,C:\QAWork | Out-Null
    Copy-Item C:\HLSSource\scripts\validation C:\HLSTests\scripts -Recurse
    $env:PYTHONPATH = 'C:\HLSSource\.tool-cache\compact-ui-validation\python-packages;C:\HLSTests\scripts\validation'
    $env:PYTHONIOENCODING = 'utf-8'
    $env:PYTHONDONTWRITEBYTECODE = '1'
    $env:HLS_VM_ACCEPTANCE = '1'
    $env:HLS_V6_SKIP_MIGRATE = '1'
    $env:TEMP = 'C:\QAWork'
    $env:TMP = $env:TEMP
    $script = 'C:\HLSTests\scripts\validation\smoke_v7_installed_footprint.py'
    $source = [IO.File]::ReadAllText($script)
    $anchor = "        extensions = {}"
    if (-not $source.Contains($anchor)) { throw 'Installed component assertion anchor is missing' }
    $extra = @'
        expected = json.loads(args.manifest.read_text(encoding='utf-8-sig'))
        actual = {relative: hashlib.sha256((install/relative).read_bytes()).hexdigest()
                  for relative in expected['components_sha256']}
        assert actual == expected['components_sha256'], 'Installed components differ from packaged source build'
        assert not (resources/'ffprobe.exe').exists()
        record('installed components, plugin ZIPs and provenance match the frozen source build; ffprobe absent', actual)
        extensions = {}
'@
    [IO.File]::WriteAllText($script, $source.Replace($anchor, $extra), [Text.UTF8Encoding]::new($false))
    'installer' | Set-Content "$work\phase.txt"
    $arguments = @($script,'--msi',"$work\candidate.msi",'--manifest',"$work\expected.json",'--report',"$work\installer.json")
    $p = Start-Process C:\HLSPython\python.exe -ArgumentList $arguments -WindowStyle Hidden -PassThru -RedirectStandardOutput "$work\installer.stdout" -RedirectStandardError "$work\installer.stderr"
    $null = $p.Handle
    if (-not $p.WaitForExit(600000)) { Stop-Process -Id $p.Id; throw 'Installer validation exceeded 10 minutes' }
    $p.ExitCode | Set-Content "$work\installer.exit"
    if ($p.ExitCode -ne 0) { throw "Installer validation failed: $($p.ExitCode)" }
    'complete' | Set-Content "$work\phase.txt"
} catch {
    $_ | Out-String | Set-Content "$work\guest.error"
} finally {
    'done' | Set-Content "$work\finished"
}
