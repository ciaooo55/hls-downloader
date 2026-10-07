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
Export-ModuleMember -Function Get-V7ReleaseExclusions
