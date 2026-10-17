param(
    [string]$ProtectedKeyPath = (Join-Path $PSScriptRoot '..\outputs\private-user-backup\qwencloud-api-key.dpapi'),
    [switch]$SaveOnly
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Security
if (Test-Path -LiteralPath $ProtectedKeyPath) {
    $protected = [IO.File]::ReadAllBytes($ProtectedKeyPath)
    $bytes = [Security.Cryptography.ProtectedData]::Unprotect($protected, $null, [Security.Cryptography.DataProtectionScope]::CurrentUser)
    $key = [Text.Encoding]::UTF8.GetString($bytes)
} else {
    if ([Console]::IsInputRedirected) { $key = [Console]::ReadLine() }
    else {
        $secure = Read-Host 'QwenCloud API Key' -AsSecureString
        $key = [Net.NetworkCredential]::new('', $secure).Password
    }
    if ([string]::IsNullOrWhiteSpace($key) -or $key.Contains("`n") -or $key.Contains("`r")) { throw 'API Key 不能为空或包含换行' }
    $bytes = [Text.Encoding]::UTF8.GetBytes($key.Trim())
    $protected = [Security.Cryptography.ProtectedData]::Protect($bytes, $null, [Security.Cryptography.DataProtectionScope]::CurrentUser)
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($ProtectedKeyPath))) | Out-Null
    [IO.File]::WriteAllBytes($ProtectedKeyPath, $protected)
}
try {
    if ($SaveOnly) { Write-Output 'QwenCloud Key 已由当前 Windows 用户加密保存。'; return }
    $pipe = [IO.Pipes.NamedPipeClientStream]::new('.', 'HLSDownloader.v7', [IO.Pipes.PipeDirection]::InOut)
    try {
        $pipe.Connect(5000)
        $payload = [Text.Encoding]::UTF8.GetBytes((@{
            type = 'subtitle'; request_id = 1
            request = @{ action = 'configure'; provider = 'qwencloud'; api_key = $key }
        } | ConvertTo-Json -Compress -Depth 4))
        $writer = [IO.BinaryWriter]::new($pipe, [Text.Encoding]::UTF8, $true)
        $reader = [IO.BinaryReader]::new($pipe, [Text.Encoding]::UTF8, $true)
        $writer.Write([uint32]$payload.Length); $writer.Write($payload); $writer.Flush()
        $length = $reader.ReadUInt32()
        if ($length -gt 1048576) { throw 'Core 响应大小无效' }
        $response = [Text.Encoding]::UTF8.GetString($reader.ReadBytes([int]$length)) | ConvertFrom-Json
        if (-not $response.result.ok -or -not $response.result.has_key -or $response.result.provider -ne 'qwencloud') { throw 'Core 未能保存 QwenCloud 配置，请确认已启动配套新版桌面端。' }
        Write-Output 'QwenCloud 已配置，Core 已加密保存 API Key。'
    } finally { $pipe.Dispose() }
} finally {
    if ($bytes) { [Array]::Clear($bytes, 0, $bytes.Length) }
    $key = $null
}
