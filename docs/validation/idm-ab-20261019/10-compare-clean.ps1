$work='C:\HLSValidation\idm-ab-20261019'
$p=Start-Process C:\HLSPython\python.exe -ArgumentList ('"'+$work+'\compare-products-clean.py"') -NoNewWindow -PassThru -RedirectStandardOutput "$work\comparison-clean.log" -RedirectStandardError "$work\comparison-clean.stderr"
$null=$p.Handle
if(-not $p.WaitForExit(900000)){Stop-Process -Id $p.Id;throw 'Comparison timed out'}
if($p.ExitCode -ne 0){throw 'Comparison fixture failed'}
$report=Get-Content "$work\comparison-clean.json" -Raw | ConvertFrom-Json
if(-not $report.passed){throw 'Comparison contains incomplete samples; inspect report'}
