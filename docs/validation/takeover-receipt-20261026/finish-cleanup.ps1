#requires -Version 7.0
[CmdletBinding(SupportsShouldProcess)]
param([switch]$Apply)
$ErrorActionPreference='Stop'
$repo=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
if(-not (Test-Path -LiteralPath "$repo\native_shell\Cargo.toml")){throw 'Project source missing'}
$delivery=Join-Path ([Environment]::GetFolderPath('Desktop')) 'HLSDownloader\最新-20261025'
$manifest=Get-Content -LiteralPath "$delivery\交付清单.json" -Raw | ConvertFrom-Json
if($manifest.verified_production_source_commit -ne '1b5efac80c91a06c23202278f0a8ae5d0b233404'){throw 'Delivery source changed'}
foreach($asset in $manifest.assets){
    $path=[IO.Path]::GetFullPath((Join-Path $delivery $asset.name))
    if(-not $path.StartsWith($delivery+'\',[StringComparison]::OrdinalIgnoreCase) -or (Get-FileHash -LiteralPath $path).Hash.ToLowerInvariant() -ne $asset.sha256){throw "Delivery mismatch: $($asset.name)"}
}
if(Get-Process WindowsSandbox,WindowsSandboxClient,HLSDownloader,HLSDownloaderEngine,HLSDownloaderNativeHost,HLSDownloaderPresenter,HLSDownloaderUpdater -ErrorAction SilentlyContinue){throw 'Sandbox or product still running'}
$relative=@('.tool-cache','extension\node_modules','extension\.output','extension\.wxt','extension\.tool-cache','scripts\validation\__pycache__')
$targets=@();$sourceLinks=@()
foreach($name in $relative){
    $target=[IO.Path]::GetFullPath((Join-Path $repo $name))
    if(-not $target.StartsWith($repo+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Target outside project'}
    $tracked=@(git -C $repo ls-files -- $name.Replace('\','/'))
    if($LASTEXITCODE -ne 0 -or $tracked.Count){throw "Tracked source in target: $name"}
    if(-not (Test-Path -LiteralPath $target)){continue}
    $parent=Get-Item -LiteralPath $target -Force
    while($parent.FullName -ne $repo){
        if($parent.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Target or ancestor is a reparse point'}
        $parent=$parent.Parent
    }
    if($name -eq '.tool-cache'){
        foreach($entry in Get-ChildItem -LiteralPath $target -Force){
            if($entry.Name -notin @('takeover-receipt-20261026','pnpm-store')){throw "Unexpected cache: $($entry.Name)"}
        }
    }
    if($name -eq 'extension\.tool-cache' -and @(Get-ChildItem -LiteralPath $target -Recurse -File -Force).Count){throw 'Expected empty accidental scaffolding'}
    if($name -eq 'scripts\validation\__pycache__'){
        foreach($entry in Get-ChildItem -LiteralPath $target -Force){
            if($entry.Name -ne 'smoke_firefox_response_takeover.cpython-312.pyc'){throw 'Unexpected Python cache'}
        }
    }
    foreach($link in Get-ChildItem -LiteralPath $target -Directory -Recurse -Force | Where-Object {$_.Attributes -band [IO.FileAttributes]::ReparsePoint}){
        $resolved=$link.ResolveLinkTarget($true)
        if($link.LinkType -eq 'SymbolicLink' -and $link.Parent.FullName -eq "$repo\.tool-cache\pnpm-store\v11\projects" -and $resolved.FullName -eq "$repo\extension"){
            $sourceLinks+=$link.FullName
        } elseif(-not $resolved -or -not $resolved.FullName.StartsWith($target+'\',[StringComparison]::OrdinalIgnoreCase)){throw "External link: $($link.FullName)"}
    }
    $targets+=$target
}
if(-not $Apply){$targets;return}
foreach($link in $sourceLinks){if($PSCmdlet.ShouldProcess($link,'Remove project-index symbolic link only')){Remove-Item -LiteralPath $link -Force}}
foreach($target in $targets){if($PSCmdlet.ShouldProcess($target,'Delete verified task-created generated directory')){Remove-Item -LiteralPath $target -Recurse -Force;Write-Host "已清理：$target"}}
$remaining=@($relative | Where-Object {Test-Path -LiteralPath (Join-Path $repo $_)})
@{passed=($remaining.Count -eq 0);remaining=$remaining;desktop_assets_verified=$manifest.assets.Count;host_install_performed=$false;registry_writes_performed=$false;backup_retained=(Test-Path "$repo\outputs\private-user-backup");checked_at=(Get-Date).ToString('o')} | ConvertTo-Json -Depth 5 | Set-Content "$PSScriptRoot\results\cleanup.json" -Encoding utf8
if($remaining.Count){throw 'Generated directories remain'}
