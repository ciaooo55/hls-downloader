#requires -Version 7.0
[CmdletBinding()]
param([switch]$Apply)
$ErrorActionPreference='Stop'
$repo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..')).TrimEnd('\')
$record=Get-Content (Join-Path $PSScriptRoot 'results/cleanup.json') -Raw | ConvertFrom-Json
if($record.path -ne '.tool-cache/full-audit-20261027'){throw 'Unexpected cleanup inventory'}
$target=[IO.Path]::GetFullPath((Join-Path $repo $record.path))
if($target -ne "$repo\.tool-cache\full-audit-20261027"){throw 'Cleanup escaped workspace'}
if(Get-Process WindowsSandbox,WindowsSandboxClient -ErrorAction SilentlyContinue){throw 'Close Sandbox first; nothing was removed'}
$active=@(Get-CimInstance Win32_Process | Where-Object {
    $_.Name -in @('java.exe','javaw.exe','javac.exe','node.exe','cargo.exe','rustc.exe','light.exe','candle.exe','HLSDownloader.exe','HLSDownloaderEngine.exe') -and
    ($_.CommandLine -like '*full-audit-20261027*' -or $_.ExecutablePath -like ($target+'\*'))
})
if($active.Count){throw 'Task build/product processes still active; nothing was removed'}
if(@(git -C $repo ls-files -- $record.path).Count -or $LASTEXITCODE -ne 0){throw 'Tracked source preserved'}
if(Test-Path -LiteralPath $target){
    $parent=Get-Item -LiteralPath $target -Force
    while($parent.FullName -ne $repo){
        if($parent.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Linked target preserved'}
        $parent=$parent.Parent
        if(-not $parent){throw 'Target escaped workspace'}
    }
    $items=@(Get-ChildItem -LiteralPath $target -Force -Recurse)
    if(@($items | Where-Object {$_.Attributes -band [IO.FileAttributes]::ReparsePoint}).Count){throw 'Cache contains links'}
    $files=@($items | Where-Object {-not $_.PSIsContainer})
    if($files.Count -ne $record.files -or @($items | Where-Object {$_.PSIsContainer}).Count -ne $record.directories -or
        [double]($files | Measure-Object Length -Sum).Sum -ne [double]$record.bytes){throw 'Cache changed since inventory; nothing was removed'}
}
$evidence=Get-Content (Join-Path $PSScriptRoot 'results/EVIDENCE-MANIFEST.json') -Raw | ConvertFrom-Json
foreach($log in $record.duplicate_logs){
    $name=[IO.Path]::GetFileName($log.path)
    if($name -notin @('render-v2-SOFTWARE.log','render-fixed-SOFTWARE.log') -or
        $log.path -ne "docs/validation/full-audit-20261027/results/$name"){throw 'Unexpected duplicate log'}
    $path=Join-Path $repo $log.path
    if(Test-Path -LiteralPath $path){
        $logParent=Get-Item -LiteralPath $path -Force
        while($logParent.FullName -ne $repo){
            if($logParent.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Linked log preserved'}
            $logParent=$logParent.Parent
            if(-not $logParent){throw 'Log escaped workspace'}
        }
        if((Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -ne $log.sha256){throw 'Duplicate log changed'}
        $replacement=Join-Path $repo $log.replacement
        $proof=@($evidence | Where-Object {$_.path -eq [IO.Path]::GetFileName($replacement)})
        if($proof.Count -ne 1 -or (Get-FileHash -LiteralPath $replacement).Hash.ToLowerInvariant() -ne $proof[0].archived_sha256){throw 'Archived log replacement is missing or changed'}
    }
}
$desktop=Join-Path $env:USERPROFILE 'Desktop\HLSDownloader\最新-20261025\HLSDownloader-7.0.3-20261027-未签名.msi'
if((Get-FileHash -LiteralPath $desktop).Hash.ToLowerInvariant() -ne $record.desktop_current_msi_sha256){throw 'Latest desktop delivery is missing or changed'}
Write-Host "$($record.path): $($record.files) files, $($record.bytes) bytes; archived evidence and desktop MSI verified."
if(-not $Apply){Write-Host '只读检查；加 -Apply 仅清理本轮缓存及两份已有归档替代的日志，不串联其他清理脚本。';return}
if(Test-Path -LiteralPath $target){Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction Stop}
foreach($log in $record.duplicate_logs){
    $path=Join-Path $repo $log.path
    if(Test-Path -LiteralPath $path){Remove-Item -LiteralPath $path -Force -ErrorAction Stop}
}
Write-Host '本轮列明缓存与重复日志已清理；源码、Git、验收证据、桌面成品、用户下载和其他目录保留。'
