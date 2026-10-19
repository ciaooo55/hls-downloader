$ErrorActionPreference='Stop'
$repo=(Resolve-Path "$PSScriptRoot\..\..").Path
$destination=Join-Path $repo 'docs\validation\targeted-performance-20261018'
New-Item -ItemType Directory -Force $destination | Out-Null
function Read-Report([string]$Name){Get-Content (Join-Path $PSScriptRoot "$Name.json") -Raw -Encoding UTF8 | ConvertFrom-Json}
$comparison=Read-Report 'comparison'
$medians=@()
foreach($count in @(10,1000)) {
    $values=@{tasks=$count}
    foreach($variant in @('baseline','candidate')) {
        $rounds=@($comparison.rounds|Where-Object {$_.tasks -eq $count -and $_.variant -eq $variant})
        if($rounds.Count -ne 3){throw 'Incomplete performance rounds'}
        $metrics=@{}
        foreach($key in @('p50_ms','p95_ms','max_ms','cpu_seconds','rss_end_mib')) {
            $metrics[$key]=@($rounds.$key|Sort-Object)[1]
        }
        $values[$variant]=$metrics
    }
    $medians+=$values
}
$sourceHashes=@{}
foreach($path in @('native_shell/src/store.rs','desktop_ui/src/main/kotlin/com/hlsdownloader/desktop/Main.kt','desktop_ui/src/test/kotlin/com/hlsdownloader/desktop/TaskIndexTest.kt','extension/lib/subtitlePopup.ts','scripts/validation/smoke_extension_subtitles.py','scripts/validation/smoke_v7_resource_boundaries.py')) {
    $sourceHashes[$path]=(Get-FileHash (Join-Path $repo $path)).Hash.ToLowerInvariant()
}
$componentDelta=0
foreach($file in Get-ChildItem "$PSScriptRoot\candidate" -File) {
    $componentDelta+=$file.Length-(Get-Item (Join-Path "$PSScriptRoot\baseline" $file.Name)).Length
}
$all=Get-Content "$PSScriptRoot\rerun-workbench-all\report.json" -Raw -Encoding UTF8 | ConvertFrom-Json
$destructive=Get-Content "$PSScriptRoot\rerun-workbench-destructive\report.json" -Raw -Encoding UTF8 | ConvertFrom-Json
if(-not $all.passed -or -not $destructive.passed){throw 'UI acceptance incomplete'}
$result=[ordered]@{
    schema=1;measured_at=[DateTime]::UtcNow.ToString('o');source_base_commit=(git -C $repo rev-parse HEAD).Trim();source_state='uncommitted local candidate'
    validation_completed=$true;functional_regressions_passed=$true;performance_improvement_demonstrated=$false;release_ready=$false
    environment=@{product_execution='Windows Sandbox only';networking='disabled; loopback fixtures only';cloud_calls=0;host_product_installations=0;new_branches=0;new_worktrees=0;new_msi=$false}
    candidate_components=(Read-Report 'components');source_hashes=$sourceHashes
    regressions=@{core=@{passed=612;failed=0;ignored=4;seconds=47.09};desktop=(Read-Report 'desktop-results');extension=@{passed=381;files=50;failed=0};builds=@('Core lib test binary','Core/NativeHost/Updater release','desktop test/JAR','Chromium','Firefox')}
    browser=@{element=(Read-Report 'subtitle-element');prefetch=(Read-Report 'subtitle-prefetch')}
    workbench=@{all=@{passed=$all.passed;checks=$all.checks};destructive=@{passed=$destructive.passed;checks=$destructive.checks}}
    performance=@{baseline='previous backend candidate, not r12';medians=$medians;raw_report='targeted-performance-20261018/comparison.json';conclusion='Mixed results: 10-task P95 decreased, 1000-task P95 and CPU increased. Overall performance benefit is not demonstrated; no causal attribution from these three rounds.'}
    component_size_delta_bytes=$componentDelta
    initial_failures=@('TypeScript TS2322 in storage.onChanged: dynamic property value was unknown. Fixed by narrowing a local variable; complete extension test/build rerun passed.','First desktop acceptance attempts were blocked by the core-tests firewall permission dialog, confirmed by screenshot. User canceled it. Remote input to relaunch tests was unreliable; a fresh Sandbox directly ran the remaining desktop scripts. No firewall setting was changed.')
    limits=@('Three alternating rounds in one Windows Sandbox, not a cross-hardware guarantee.','Core benchmark covers 1000 IPC progress commits after event-ring warmup, not desktop frame latency or Internet throughput.','No real cloud provider, physical casting/TVBox, multi-hour soak, new MSI installation, signing, GitHub push or release.','Candidate remains local and uncommitted; previous reports still apply only to their recorded hashes.')
}
$result|ConvertTo-Json -Depth 25|Set-Content (Join-Path $repo 'docs\validation\validation-targeted-performance-20261018.json') -Encoding UTF8
Get-ChildItem $PSScriptRoot -File | Where-Object {$_.Extension -in @('.log','.stderr','.exit','.json','.ps1','.py','.wsb') -and $_.Name -ne 'core-compile.jsonl'} | Copy-Item -Destination $destination -Force
foreach($name in @('workbench-all','workbench-destructive','rerun-workbench-all','rerun-workbench-destructive')) {
    New-Item -ItemType Directory -Force (Join-Path $destination $name)|Out-Null
    Get-ChildItem (Join-Path $PSScriptRoot $name) -File | Where-Object {$_.Extension -in @('.json','.png','.log','.stderr')} | Copy-Item -Destination (Join-Path $destination $name) -Force
}
$testResults=Join-Path $repo '.tool-cache\build-cache\compose-build\test-results\test'
New-Item -ItemType Directory -Force "$destination\desktop-tests"|Out-Null
Copy-Item "$testResults\TEST-*.xml" "$destination\desktop-tests"
