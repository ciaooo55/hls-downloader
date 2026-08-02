param(
    [Parameter(Mandatory = $true)]
    [string]$ArchivePath
)

$ErrorActionPreference = "Stop"

function Write-Utf8NoBom([string]$Path, [string]$Value) {
    [System.IO.File]::WriteAllText(
        $Path,
        $Value,
        (New-Object System.Text.UTF8Encoding($false))
    )
}

$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$archive = (Resolve-Path $ArchivePath).Path
$smokeRoot = Join-Path $root "build\upgrade-smoke"
$rootPrefix = [IO.Path]::GetFullPath($root)
$directorySeparator = [string][IO.Path]::DirectorySeparatorChar
if (-not $rootPrefix.EndsWith($directorySeparator, [System.StringComparison]::Ordinal)) {
    $rootPrefix += $directorySeparator
}
if (-not [IO.Path]::GetFullPath($smokeRoot).StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to use an upgrade smoke path outside the project: $smokeRoot"
}

$source = Join-Path $smokeRoot "source"
$target = Join-Path $smokeRoot "target"
$registryPrefix = "HKCU:\Software\HLSDownloaderPortableUpgradeSmoke"

function Get-NonSmokeApplicationProcessIds {
    $smokePrefix = [IO.Path]::GetFullPath($smokeRoot)
    if (-not $smokePrefix.EndsWith($directorySeparator, [System.StringComparison]::Ordinal)) {
        $smokePrefix += $directorySeparator
    }
    return @(
        Get-Process HLSDownloader,HLSDownloaderCore -ErrorAction SilentlyContinue |
        Where-Object {
            try {
                -not $_.Path -or -not [IO.Path]::GetFullPath($_.Path).StartsWith(
                    $smokePrefix,
                    [System.StringComparison]::OrdinalIgnoreCase
                )
            } catch {
                $true
            }
        } |
        Select-Object -ExpandProperty Id
    )
}

try {
    Remove-Item -LiteralPath $smokeRoot -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $smokeRoot -Force | Out-Null
    Expand-Archive -LiteralPath $archive -DestinationPath $source -Force
    Copy-Item -LiteralPath $source -Destination $target -Recurse -Force

    $targetConfig = [ordered]@{
        config_version = 18
        token = "preserved-token-for-upgrade-smoke-0123456789"
        port = 29991
        download_dir = "downloads"
        temp_dir = "."
        ffmpeg_path = "bin\\ffmpeg.exe"
    } | ConvertTo-Json
    Write-Utf8NoBom (Join-Path $target "config.json") $targetConfig
    Set-Content -LiteralPath (Join-Path $target "data.db") -Value "persistent task database" -Encoding UTF8
    New-Item -ItemType Directory -Path (Join-Path $target "downloads") -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $target "downloads\keep.txt") -Value "do not replace" -Encoding UTF8

    $officialBefore = Get-NonSmokeApplicationProcessIds
    & (Join-Path $source "scripts\upgrade-portable.ps1") `
        -TargetDir $target `
        -RegistryPrefix $registryPrefix `
        -DeleteSourceAfterUpgrade
    if ($LASTEXITCODE -ne 0) { throw "Portable upgrade script returned $LASTEXITCODE" }
    if (Test-Path -LiteralPath $source) {
        throw "Portable upgrade did not remove its extracted update staging directory"
    }

    if ((Get-Content -LiteralPath (Join-Path $target "config.json") -Raw -Encoding UTF8) -notmatch "preserved-token-for-upgrade-smoke") {
        throw "Portable upgrade did not preserve config.json"
    }
    if ((Get-Content -LiteralPath (Join-Path $target "data.db") -Raw -Encoding UTF8) -notmatch "persistent task database") {
        throw "Portable upgrade did not preserve data.db"
    }
    if ((Get-Content -LiteralPath (Join-Path $target "downloads\keep.txt") -Raw -Encoding UTF8) -notmatch "do not replace") {
        throw "Portable upgrade did not preserve downloads"
    }
    $versionedHosts = @(Get-ChildItem -LiteralPath (Join-Path $target "native-host\versions") -Filter "HLSDownloaderNativeHost-*.exe" -File)
    if ($versionedHosts.Count -ne 1 -or $versionedHosts[0].BaseName -notmatch '^HLSDownloaderNativeHost-(?<version>\d+(?:\.\d+){1,3})$') {
        throw "Portable upgrade must contain exactly one versioned Native Host"
    }
    $packageVersion = $Matches.version
    if (-not (Test-Path -LiteralPath (Join-Path $target "native-host\manifests\chrome-$packageVersion.json"))) {
        throw "Portable upgrade is missing the matching Chromium Native Host manifest"
    }
    foreach ($required in @(
        "HLSDownloader.exe",
        "HLSDownloaderCore.exe",
        "scripts\upgrade-portable.ps1",
        "scripts\shutdown-running.ps1",
        "portable"
    )) {
        if (-not (Test-Path -LiteralPath (Join-Path $target $required))) {
            throw "Portable upgrade output is missing: $required"
        }
    }

    $registered = Get-Item -LiteralPath "$registryPrefix\Google\Chrome\NativeMessagingHosts\com.ciaooo55.hls_downloader" -ErrorAction Stop
    if (-not $registered.GetValue("")) { throw "Portable upgrade did not register the temporary Native Host" }

    $officialAfter = Get-NonSmokeApplicationProcessIds
    if ($officialBefore.Count -gt 0 -and $officialAfter.Count -lt $officialBefore.Count) {
        throw "Portable upgrade interfered with the installed app"
    }

    [PSCustomObject]@{
        PreservedConfig = $true
        PreservedDatabase = $true
        PreservedDownloads = $true
        VersionedNativeHost = $true
        TemporaryBrowserRegistration = $true
        OfficialAppProcessCountBefore = $officialBefore.Count
        OfficialAppProcessCountAfter = $officialAfter.Count
    }
} finally {
    Remove-Item -LiteralPath $registryPrefix -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $smokeRoot -Recurse -Force -ErrorAction SilentlyContinue
}
