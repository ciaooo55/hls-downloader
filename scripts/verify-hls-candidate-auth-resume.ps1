[CmdletBinding()]
param(
    [string]$CandidateZip = '',
    [string]$ReportPath = 'artifacts\v7-productization\hls-candidate-auth-resume-evidence.txt',
    [string]$PythonPath = '',
    [ValidateRange(1, 5)]
    [int]$Runs = 1
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$report = New-Object System.Collections.Generic.List[string]
$overall = 0
$runRoot = Join-Path ([IO.Path]::GetTempPath()) ('hls-candidate-auth-' + [guid]::NewGuid().ToString('n'))
$extractRoot = Join-Path $runRoot 'portable'
$candidateRoot = Join-Path $root 'artifacts\v7-productization\candidate'
if ([String]::IsNullOrWhiteSpace($CandidateZip)) {
    $CandidateZip = Join-Path $candidateRoot 'HLSDownloader-7.0.0-Windows-x64-Portable-candidate.zip'
}
$CandidateZip = [IO.Path]::GetFullPath($CandidateZip)
if (-not (Test-Path -LiteralPath $CandidateZip -PathType Leaf)) {
    throw "Candidate Portable ZIP was not found: $CandidateZip"
}
if ([String]::IsNullOrWhiteSpace($PythonPath)) {
    $pythonCommand = Get-Command python.exe -ErrorAction SilentlyContinue
    if ($null -ne $pythonCommand) { $PythonPath = $pythonCommand.Source }
    else { $PythonPath = 'C:\Users\lee\.conda\envs\test\python.exe' }
}
$PythonPath = [IO.Path]::GetFullPath($PythonPath)
if (-not (Test-Path -LiteralPath $PythonPath -PathType Leaf)) {
    throw "python.exe was not found: $PythonPath"
}
$fixture = Join-Path $root 'scripts\fixtures\hls_candidate_auth_server.py'
$report.Add('Candidate Portable authenticated HLS pause-resume verification')
$report.Add("UTC date: $([DateTime]::UtcNow.ToString('o'))")
$report.Add("Candidate ZIP: $CandidateZip")
$report.Add("Fixture: $fixture")
$report.Add("Python: $PythonPath")
$report.Add("Runs: $Runs")

function Get-FreeTcpPort {
    $listener = New-Object System.Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try { return $listener.LocalEndpoint.Port } finally { $listener.Stop() }
}

function Read-Exact([System.Net.Sockets.NetworkStream]$Stream, [int]$Count) {
    $buffer = New-Object byte[] $Count
    $offset = 0
    while ($offset -lt $Count) {
        $read = $Stream.Read($buffer, $offset, $Count - $offset)
        if ($read -le 0) { throw 'Core TCP connection closed while reading a frame.' }
        $offset += $read
    }
    return $buffer
}

function Send-CoreRequest([System.Net.Sockets.TcpClient]$Client, $Request) {
    $json = ConvertTo-Json -InputObject $Request -Compress -Depth 30
    $payload = [Text.Encoding]::UTF8.GetBytes($json)
    $length = [BitConverter]::GetBytes([uint32]$payload.Length)
    $stream = $Client.GetStream()
    $stream.Write($length, 0, $length.Length)
    $stream.Write($payload, 0, $payload.Length)
    $header = Read-Exact $stream 4
    $responseLength = [BitConverter]::ToUInt32($header, 0)
    if ($responseLength -gt 4194304) { throw "Core response frame too large: $responseLength" }
    $response = Read-Exact $stream ([int]$responseLength)
    return ([Text.Encoding]::UTF8.GetString($response) | ConvertFrom-Json)
}

function Connect-Core([string]$Address) {
    $parts = $Address.Split(':')
    $client = New-Object System.Net.Sockets.TcpClient
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    while ($true) {
        try {
            $client.Connect($parts[0], [int]$parts[1])
            break
        } catch {
            if ([DateTime]::UtcNow -ge $deadline) { throw "Candidate Engine Core did not accept TCP: $Address" }
            Start-Sleep -Milliseconds 100
        }
    }
    $hello = [ordered]@{ type = 'hello'; protocol = 'hls-downloader-v7-core'; version = 1 }
    $response = Send-CoreRequest $client $hello
    if ($response.type -ne 'hello' -or $response.protocol -ne 'hls-downloader-v7-core') {
        throw "Unexpected Core hello response: $($response | ConvertTo-Json -Compress)"
    }
    return $client
}

function Get-HttpStatus([string]$Url) {
    $request = [Net.HttpWebRequest]::Create($Url)
    $request.Method = 'GET'
    try {
        $response = [Net.HttpWebResponse]$request.GetResponse()
        try { return [int]$response.StatusCode } finally { $response.Dispose() }
    } catch [Net.WebException] {
        if ($null -ne $_.Exception.Response) {
            return [int]([Net.HttpWebResponse]$_.Exception.Response).StatusCode
        }
        throw
    }
}

function Wait-File([string]$Path, [int]$Seconds, [string]$Message) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        if ([DateTime]::UtcNow -ge $deadline) { throw $Message }
        Start-Sleep -Milliseconds 50
    }
}

function Wait-Snapshot($Client, [string]$TaskId, [string[]]$Statuses, [int]$Seconds) {
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    do {
        $request = [ordered]@{ type = 'snapshot'; request_id = 70 }
        $response = Send-CoreRequest $Client $request
        $task = @($response.tasks) | Where-Object { $_.task_id -eq $TaskId } | Select-Object -First 1
        if ($null -ne $task -and $Statuses -contains [string]$task.status -and
            ([string]$task.status -ne 'paused' -or [int]$task.active_workers -eq 0)) { return $task }
        if ($null -ne $task) { $currentStatus = [string]$task.status } else { $currentStatus = 'missing' }
        if ($null -ne $task -and $currentStatus -eq 'failed') {
            throw "Candidate task failed: $($task.error_code) $($task.error_message)"
        }
        if ([DateTime]::UtcNow -ge $deadline) { throw "Task $TaskId did not reach $($Statuses -join ',') (last=$currentStatus)" }
        Start-Sleep -Milliseconds 100
    } while ($true)
}

function Invoke-CandidateCase([string]$Mode, [int]$Ordinal, [string]$Engine, [string]$DataRoot) {
    $caseRoot = Join-Path $runRoot "$Mode-$Ordinal"
    $serverRoot = Join-Path $caseRoot 'server'
    $downloadRoot = Join-Path $caseRoot 'downloads'
    $workRoot = Join-Path $caseRoot 'work'
    New-Item -ItemType Directory -Force -Path $serverRoot,$downloadRoot,$workRoot | Out-Null
    $serverArgs = @('-u', $fixture, '--mode', $Mode, '--run-root', $serverRoot)
    $server = Start-Process -FilePath $PythonPath -ArgumentList $serverArgs -WorkingDirectory $root -WindowStyle Hidden -PassThru
    try {
        Wait-File (Join-Path $serverRoot 'http-port.txt') 15 "HLS fixture did not publish its port for $Mode."
        $httpPort = [int](Get-Content -LiteralPath (Join-Path $serverRoot 'http-port.txt') -Raw -Encoding UTF8).Trim()
        $url = "http://127.0.0.1:$httpPort/$Mode.m3u8"
        $unauthorizedStatus = Get-HttpStatus $url
        if ($unauthorizedStatus -ne 401) { throw "Unauthenticated $Mode playlist returned $unauthorizedStatus, expected 401." }
        $corePort = Get-FreeTcpPort
        $env:HLS_V7_CORE_TCP = '1'
        $env:HLS_V7_CORE_BIND = "127.0.0.1:$corePort"
        $env:HLS_V7_DATA_DIR = (Join-Path $caseRoot 'data')
        New-Item -ItemType Directory -Force -Path $env:HLS_V7_DATA_DIR | Out-Null
        $engineStdout = Join-Path $caseRoot 'engine.stdout.log'
        $engineStderr = Join-Path $caseRoot 'engine.stderr.log'
        $engineProcess = Start-Process -FilePath $Engine -WorkingDirectory (Split-Path $Engine -Parent) -WindowStyle Hidden -RedirectStandardOutput $engineStdout -RedirectStandardError $engineStderr -PassThru
        $client = $null
        try {
            try {
                $client = Connect-Core "127.0.0.1:$corePort"
            } catch {
                $stdout = if (Test-Path -LiteralPath $engineStdout) { (Get-Content -LiteralPath $engineStdout -Raw -Encoding UTF8).Trim() } else { '' }
                $stderr = if (Test-Path -LiteralPath $engineStderr) { (Get-Content -LiteralPath $engineStderr -Raw -Encoding UTF8).Trim() } else { '' }
                throw "Core connect failed (engine_exit=$($engineProcess.HasExited), stdout=$stdout, stderr=$stderr): $($_.Exception.Message)"
            }
            $settings = [ordered]@{ type = 'store_setting'; request_id = 71; key = 'download_dir'; value = $downloadRoot }
            $settingsResponse = Send-CoreRequest $client $settings
            if ($settingsResponse.type -eq 'error') { throw "Setting download_dir failed: $($settingsResponse.message)" }
            $settings = [ordered]@{ type = 'store_setting'; request_id = 72; key = 'temp_dir'; value = $workRoot }
            $settingsResponse = Send-CoreRequest $client $settings
            if ($settingsResponse.type -eq 'error') { throw "Setting temp_dir failed: $($settingsResponse.message)" }
            $filename = "candidate-$Mode-$Ordinal.ts"
            $spec = [ordered]@{
                url = $url
                resource_kind = if ($Mode -eq 'vod') { 'hls' } else { 'live' }
                title = "Candidate authenticated $Mode"
                filename = $filename
                download_dir = $downloadRoot
                work_dir = $workRoot
                request_method = 'GET'
                concurrency = 1
                headers = [ordered]@{ Authorization = 'Bearer hls-v7-candidate' }
                allow_duplicate = $true
                queue_id = 'default'
            }
            $create = [ordered]@{ type = 'command'; request_id = 73; command = [ordered]@{ kind = 'create_task'; spec = $spec } }
            $createResponse = Send-CoreRequest $client $create
            if ($createResponse.type -eq 'error') { throw "CreateTask failed: $($createResponse.message)" }
            $created = @($createResponse.events) | Where-Object { $_.'event'.kind -eq 'task_created' } | Select-Object -First 1
            if ($null -eq $created) { throw "CreateTask did not return task_created: $($createResponse | ConvertTo-Json -Compress -Depth 20)" }
            $taskId = [string]$created.'event'.snapshot.task_id
            $start = [ordered]@{ type = 'command'; request_id = 74; command = [ordered]@{ kind = 'task_action'; task_id = $taskId; action = 'start' } }
            $startResponse = Send-CoreRequest $client $start
            if ($startResponse.type -eq 'error') { throw "Task start failed: $($startResponse.message)" }
            Wait-File (Join-Path $serverRoot 'first-segment.seen') 20 "Candidate Engine did not request the first $Mode segment."
            $pause = [ordered]@{ type = 'command'; request_id = 75; command = [ordered]@{ kind = 'task_action'; task_id = $taskId; action = 'pause' } }
            $pauseResponse = Send-CoreRequest $client $pause
            if ($pauseResponse.type -eq 'error') { throw "Task pause failed: $($pauseResponse.message)" }
            Set-Content -LiteralPath (Join-Path $serverRoot 'first-segment.release') -Value 'release' -Encoding ASCII
            $paused = Wait-Snapshot $client $taskId @('paused') 20
            # The pause event is published before the worker thread removes its
            # active registration. Let that transition settle before resume.
            Start-Sleep -Milliseconds 750
            $checkpoint = Join-Path (Join-Path (Join-Path $workRoot '.hls-tasks') $taskId) 'live_state.json'
            $checkpointState = if ($Mode -eq 'live' -and (Test-Path -LiteralPath $checkpoint)) { 'present' } elseif ($Mode -eq 'live') { 'missing' } else { 'not_applicable' }
            $resume = [ordered]@{ type = 'command'; request_id = 76; command = [ordered]@{ kind = 'task_action'; task_id = $taskId; action = 'resume' } }
            $resumeResponse = Send-CoreRequest $client $resume
            if ($resumeResponse.type -eq 'error') { throw "Task resume failed: $($resumeResponse.message)" }
            $completed = Wait-Snapshot $client $taskId @('completed') 45
            $output = Join-Path $downloadRoot $filename
            if (-not (Test-Path -LiteralPath $output -PathType Leaf)) { throw "Candidate output is missing: $output" }
            $body = [IO.File]::ReadAllBytes($output)
            $expected = if ($Mode -eq 'vod') { 'CANDIDATE-VOD-0CANDIDATE-VOD-1' } else { 'CANDIDATE-LIVE-0CANDIDATE-LIVE-1' }
            $actual = [Text.Encoding]::ASCII.GetString($body)
            if ($actual -ne $expected) { throw "Candidate $Mode output mismatch: $actual" }
            $rows = @(Get-Content -LiteralPath (Join-Path $serverRoot 'requests.jsonl') -Encoding UTF8 | ForEach-Object { $_ | ConvertFrom-Json })
            $firstPath = "/$Mode/" + $(if ($Mode -eq 'vod') { 'a.ts' } else { '0.ts' })
            $secondPath = "/$Mode/" + $(if ($Mode -eq 'vod') { 'b.ts' } else { '1.ts' })
            $firstCount = @($rows | Where-Object { $_.path -eq $firstPath -and $_.authorized -eq $true }).Count
            $secondCount = @($rows | Where-Object { $_.path -eq $secondPath -and $_.authorized -eq $true }).Count
            $unauthorizedCount = @($rows | Where-Object { $_.authorized -eq $false -and $_.status -eq 401 }).Count
            if ($firstCount -ne 1 -or $secondCount -ne 1 -or $unauthorizedCount -ne 1) {
                throw "Request evidence mismatch: first=$firstCount second=$secondCount unauthorized=$unauthorizedCount"
            }
            $authRows = @($rows | Where-Object { $_.status -ge 200 -and $_.status -lt 400 -and $_.authorized -ne $true })
            if ($authRows.Count -ne 0) { throw 'A successful HLS request did not carry Authorization.' }
            $serverRows = ($rows | ForEach-Object { "{0}|auth={1}|status={2}" -f $_.path,$_.authorized,$_.status }) -join ';'
            $result = [ordered]@{
                mode = $Mode
                task_id = $taskId
                unauthorized_status = $unauthorizedStatus
                paused_status = [string]$paused.status
                resumed_status = [string]$completed.status
                checkpoint = $checkpointState
                output_bytes = $body.Length
                first_segment_requests = $firstCount
                second_segment_requests = $secondCount
                unauthorized_requests = $unauthorizedCount
                request_trace = $serverRows
            }
            return $result
        } finally {
            if ($null -ne $client) { $client.Close() }
            if ($null -ne $engineProcess -and -not $engineProcess.HasExited) {
                Stop-Process -Id $engineProcess.Id -Force -ErrorAction SilentlyContinue
                if ($null -ne $engineProcess) { $null = $engineProcess.WaitForExit() }
            }
        }
    } finally {
        New-Item -ItemType File -Force -Path (Join-Path $serverRoot 'server.stop') | Out-Null
        if ($null -ne $server -and -not $server.HasExited) {
            if ($null -ne $server) {
                $null = $server.WaitForExit(5000)
                if (-not $server.HasExited) { Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue }
            }
        }
    }
}

$oldEnvironment = @{}
foreach ($key in @('HLS_V7_CORE_TCP','HLS_V7_CORE_BIND','HLS_V7_DATA_DIR')) {
    $oldEnvironment[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
}
$engine = $null
try {
    New-Item -ItemType Directory -Force -Path $runRoot | Out-Null
    New-Item -ItemType Directory -Force -Path $extractRoot | Out-Null
    Expand-Archive -LiteralPath $CandidateZip -DestinationPath $extractRoot -Force
    $engine = Join-Path $extractRoot 'HLSDownloader\app\resources\HLSDownloaderEngine.exe'
    if (-not (Test-Path -LiteralPath $engine -PathType Leaf)) { throw "Portable candidate Engine is missing: $engine" }
    for ($run = 1; $run -le $Runs; $run++) {
        foreach ($mode in @('vod','live')) {
            try {
                $result = Invoke-CandidateCase $mode $run $engine (Join-Path $runRoot 'data')
                $report.Add((ConvertTo-Json -InputObject $result -Compress -Depth 8))
                $report.Add("RESULT: $mode run $run passed")
            } catch {
                $overall = 1
                $caseRoot = Join-Path $runRoot "$mode-$run"
                $stdoutPath = Join-Path $caseRoot 'engine.stdout.log'
                $stderrPath = Join-Path $caseRoot 'engine.stderr.log'
                $stdoutRaw = if (Test-Path -LiteralPath $stdoutPath) { Get-Content -LiteralPath $stdoutPath -Raw -Encoding UTF8 } else { $null }
                $stderrRaw = if (Test-Path -LiteralPath $stderrPath) { Get-Content -LiteralPath $stderrPath -Raw -Encoding UTF8 } else { $null }
                $stdout = if ($null -eq $stdoutRaw) { '' } else { ([string]$stdoutRaw).Trim() }
                $stderr = if ($null -eq $stderrRaw) { '' } else { ([string]$stderrRaw).Trim() }
                $report.Add("RESULT: $mode run $run failed: $($_.Exception.Message); engine_stdout=$stdout; engine_stderr=$stderr")
            }
        }
    }
    if ($overall -eq 0) { $report.Add('RESULT: candidate Portable Engine authenticated VOD and Live pause-resume verification passed') }
    else { $report.Add('RESULT: candidate Portable Engine verification failed') }
} finally {
    foreach ($key in $oldEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($key, $oldEnvironment[$key], 'Process')
    }
}
$reportFullPath = if ([IO.Path]::IsPathRooted($ReportPath)) { [IO.Path]::GetFullPath($ReportPath) } else { [IO.Path]::GetFullPath((Join-Path $root $ReportPath)) }
$parent = Split-Path -Parent $reportFullPath
if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
[IO.File]::WriteAllText($reportFullPath, ($report -join [Environment]::NewLine), $utf8NoBom)
if (Test-Path -LiteralPath $runRoot) { Remove-Item -LiteralPath $runRoot -Recurse -Force -ErrorAction SilentlyContinue }
if ($overall -ne 0) { exit $overall }
