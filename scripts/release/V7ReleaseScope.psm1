. (Join-Path $PSScriptRoot '..\V7HashFunctions.ps1')

function Get-V7ReleaseExclusions($Feature) {
    $allowedFeatures = @('media.cast_dlna_chromecast', 'media.tvbox_push', 'media.cast_and_player_concurrency', 'browser.media_push_device_selection', 'browser.online_subtitle_translation')
    $features = @(); $gates = @(); $reasons = @{}
    foreach ($exclusion in @($Feature.release_exclusions)) {
        if ($null -eq $exclusion) { continue }
        if ([String]::IsNullOrWhiteSpace([string]$exclusion.reason)) { throw 'Release exclusions require an explicit reason.' }
        foreach ($id in @($exclusion.feature_ids)) {
            if ($allowedFeatures -notcontains $id -or $features -contains $id) { throw "Unsupported or duplicate release feature exclusion: $id" }
            $features += $id
        }
        foreach ($id in @($exclusion.gate_ids)) {
            if ($id -ne 'browser_media_push' -or $gates -contains $id -or $exclusion.feature_ids -notcontains 'browser.media_push_device_selection') {
                throw "Unsupported or duplicate release gate exclusion: $id"
            }
            $gates += $id; $reasons[$id] = [string]$exclusion.reason
        }
    }
    [pscustomobject]@{ features = $features; gates = $gates; reasons = $reasons }
}

function Resolve-V7PackageFile([string]$Root, [string]$RelativePath, [string]$ExpectedHash) {
    if ([string]::IsNullOrWhiteSpace($RelativePath) -or [IO.Path]::IsPathRooted($RelativePath) -or
        $ExpectedHash -notmatch '\A[0-9a-fA-F]{64}\z') { throw "Invalid package entry: $RelativePath" }
    $path = $Root
    foreach ($part in $RelativePath.Split([char[]]'\/')) {
        if (-not $part -or $part.EndsWith('.') -or $part.EndsWith(' ') -or $part.IndexOfAny([IO.Path]::GetInvalidFileNameChars()) -ge 0) {
            throw "Invalid package path: $RelativePath"
        }
        $path = [IO.Path]::Combine($path, $part)
        if ([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) {
            throw "Package path crosses a reparse point: $RelativePath"
        }
    }
    if (-not [IO.File]::Exists($path) -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $ExpectedHash) {
        throw "Package SHA-256 mismatch: $RelativePath"
    }
    return $path
}

function Get-V7PortableExecutables {
    'HLSDownloader.exe'
    foreach ($name in @('Engine', 'NativeHost', 'Updater', 'Presenter', 'Cli')) {
        "app/resources/HLSDownloader$name.exe"
    }
}

Export-ModuleMember -Function Get-V7ReleaseExclusions, Resolve-V7PackageFile, Get-V7PortableExecutables
