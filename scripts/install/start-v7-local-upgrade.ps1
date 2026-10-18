[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$PythonPath,
    [Parameter(Mandatory = $true)][string]$Evidence
)
$ErrorActionPreference = 'Stop'
$python = (Resolve-Path -LiteralPath $PythonPath).Path
$evidencePath = (Resolve-Path -LiteralPath $Evidence).Path
$script = Join-Path $PSScriptRoot '..\validation\verify_v7_local_upgrade.py'
$arguments = '"{0}" --evidence "{1}"' -f $script, $evidencePath
# 独立进程负责整个安装事务，聊天回复或观察命令退出不会提前重启 Core。
$worker = Start-Process -FilePath $python -ArgumentList $arguments -WindowStyle Hidden -PassThru `
    -RedirectStandardOutput (Join-Path $evidencePath 'local-upgrade-worker.log') `
    -RedirectStandardError (Join-Path $evidencePath 'local-upgrade-worker-error.log')
[ordered]@{ worker_pid = $worker.Id; report = (Join-Path $evidencePath 'installed-verification.json') } | ConvertTo-Json
