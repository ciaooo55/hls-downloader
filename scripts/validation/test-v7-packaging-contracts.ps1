[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
. (Join-Path $repo 'scripts\build\V7MsiIdentity.ps1')
Import-Module (Join-Path $repo 'scripts\release\V7ReleaseScope.psm1') -Force

$guidPattern = '^\{[0-9A-F]{8}-[0-9A-F]{4}-5[0-9A-F]{3}-[89AB][0-9A-F]{3}-[0-9A-F]{12}\}$'

# 1. CR-P1-3: Same commit candidate vs formal MSI must always get distinct ProductCodes,
#    and repeated builds with default nonce must also receive distinct ProductCodes.
$commit = 'a' * 40
$candidateCode = New-V7MsiProductCode -SourceCommit $commit -PackageTier 'candidate' -ProductVersion '10.1.0' -BuildNonce 'fixed-nonce'
$formalCode = New-V7MsiProductCode -SourceCommit $commit -PackageTier 'formal' -ProductVersion '10.1.0' -BuildNonce 'fixed-nonce'
if ($candidateCode -notmatch $guidPattern -or $formalCode -notmatch $guidPattern) {
    throw "Invalid MSI ProductCode format: candidate=$candidateCode formal=$formalCode"
}
if ($candidateCode -eq $formalCode) {
    throw "Candidate and formal MSI from the same commit must not share a ProductCode: $candidateCode"
}
$firstDefault = New-V7MsiProductCode -SourceCommit $commit -PackageTier 'candidate' -ProductVersion '10.1.0'
$secondDefault = New-V7MsiProductCode -SourceCommit $commit -PackageTier 'candidate' -ProductVersion '10.1.0'
if ($firstDefault -eq $secondDefault) {
    throw "Repeated MSI builds on the same commit must receive distinct ProductCodes: $firstDefault"
}

# 2. CR-P1-4: Signing scope must cover all 6 MSI EXEs plus HLSDownloaderPreflight.exe,
#    and signing scripts must wire pre-bundle MSI signing, Burn insignia signing, and MSI inner checks.
$msiExes = @(Get-V7MsiFirstPartyExecutables)
$portableExes = @(Get-V7PortableExecutables)
if ($msiExes.Count -ne 6) {
    throw "Expected 6 MSI first-party executables, found $($msiExes.Count): $($msiExes -join ', ')"
}
if ($portableExes -notcontains 'scripts/HLSDownloaderPreflight.exe' -or $portableExes.Count -ne 7) {
    throw "Get-V7PortableExecutables must include scripts/HLSDownloaderPreflight.exe (7 total); found: $($portableExes -join ', ')"
}
$installerSource = Get-Content -LiteralPath (Join-Path $repo 'scripts\build\build-native-v7-installer.ps1') -Raw -Encoding UTF8
foreach ($needle in @('Invoke-InstallerSign @($OutMsi)', 'insignia.exe', '-ib $OutExe', '-ab $burnEngine $OutExe')) {
    if ($installerSource -notmatch [regex]::Escape($needle)) {
        throw "build-native-v7-installer.ps1 is missing signing chain step: $needle"
    }
}
$finalizeSource = Get-Content -LiteralPath (Join-Path $repo 'scripts\release\finalize-v7-signed-package.ps1') -Raw -Encoding UTF8
foreach ($needle in @('build-native-v7-installer.ps1', 'New-V7MsiProductCode', 'HLSDownloaderPreflight.exe')) {
    if ($finalizeSource -notmatch [regex]::Escape($needle)) {
        throw "finalize-v7-signed-package.ps1 is missing signed rebuild wiring: $needle"
    }
}
$verifyAuthSource = Get-Content -LiteralPath (Join-Path $repo 'scripts\release\verify-v7-authenticode.ps1') -Raw -Encoding UTF8
foreach ($needle in @('Get-V7MsiFirstPartyExecutables', 'Export-MsiBinaryStream', 'V7InstallerHelper', 'insignia.exe', '@([int]1, [int]$chunkSize, [int]1)')) {
    if ($verifyAuthSource -notmatch [regex]::Escape($needle)) {
        throw "verify-v7-authenticode.ps1 is missing inner binary/Burn verification: $needle"
    }
}
$buildSource = Get-Content -LiteralPath (Join-Path $repo 'scripts\build-v7.ps1') -Raw -Encoding UTF8
if ($buildSource -notmatch 'foreach \(\$name in Get-V7PortableExecutables\)') {
    throw 'build-v7.ps1 formal candidate promotion must require every Get-V7PortableExecutables component (including scripts/HLSDownloaderPreflight.exe).'
}

# 2c. PRB-P1-3: Installed MSI directory verification and diagnostics must scope to
#     Get-V7MsiFirstPartyExecutables (6 installed EXEs), because scripts/HLSDownloaderPreflight.exe
#     lives only in the Portable ZIP and the MSI Binary table (not INSTALLDIR).
$exeStartupSource = Get-Content -LiteralPath (Join-Path $repo 'scripts\validation\verify-v7-exe-startup.ps1') -Raw -Encoding UTF8
foreach ($needle in @('V7ReleaseScope.psm1', 'Get-V7MsiFirstPartyExecutables', 'Get-V7PortableExecutables')) {
    if ($exeStartupSource -notmatch [regex]::Escape($needle)) {
        throw "verify-v7-exe-startup.ps1 is missing MSI component scope guard: $needle"
    }
}
if ($exeStartupSource -match 'Join-Path \$ownedInstallDir \$component\.Name') {
    throw 'verify-v7-exe-startup.ps1 must not hash non-MSI manifest.components entries against $ownedInstallDir.'
}
$msiLifecycleSource = Get-Content -LiteralPath (Join-Path $repo 'scripts\validation\verify-v7-msi-lifecycle.ps1') -Raw -Encoding UTF8
foreach ($needle in @('V7ReleaseScope.psm1', 'foreach ($name in Get-V7MsiFirstPartyExecutables)')) {
    if ($msiLifecycleSource -notmatch [regex]::Escape($needle)) {
        throw "verify-v7-msi-lifecycle.ps1 is missing MSI installed_components diagnostic scope: $needle"
    }
}
if ($msiLifecycleSource -match '\$manifest\.components\.PSObject\.Properties') {
    throw 'verify-v7-msi-lifecycle.ps1 must not iterate $manifest.components.PSObject.Properties against an MSI install directory.'
}

# 2b. PRB-P1-1: Verify WindowsInstaller.Installer Binary ReadStream format 1 (msiReadStreamBytes)
#     preserves all 256 byte values (0x00..0xFF) losslessly under both PowerShell 5.1 and 7.
$msiStreamTemp = Join-Path (Join-Path $repo '.tool-cache\test-tmp') ('msi-binary-stream-' + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force -Path $msiStreamTemp | Out-Null
try {
    $fixtureBytes = New-Object byte[] 4096
    for ($i = 0; $i -lt $fixtureBytes.Length; $i++) {
        $fixtureBytes[$i] = [byte]($i % 256)
    }
    $fixtureBin = Join-Path $msiStreamTemp 'fixture.bin'
    $fixtureMsi = Join-Path $msiStreamTemp 'fixture.msi'
    $exportedBin = Join-Path $msiStreamTemp 'exported.bin'
    [IO.File]::WriteAllBytes($fixtureBin, $fixtureBytes)
    $expectedFixtureHash = (Get-FileHash -LiteralPath $fixtureBin -Algorithm SHA256).Hash.ToLowerInvariant()

    $installer = $null
    $database = $null
    $view = $null
    $record = $null
    try {
        $installer = New-Object -ComObject WindowsInstaller.Installer
        # Mode 3 = msiOpenDatabaseModeCreate
        $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @([string]$fixtureMsi, [int]3))
        $createView = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @('CREATE TABLE `Binary` (`Name` CHAR(72) NOT NULL, `Data` OBJECT PRIMARY KEY `Name`)'))
        try {
            $createView.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $createView, $null) | Out-Null
        } finally {
            [Runtime.InteropServices.Marshal]::FinalReleaseComObject($createView) | Out-Null
        }
        $record = $installer.GetType().InvokeMember('CreateRecord', 'InvokeMethod', $null, $installer, @([int]2))
        $record.GetType().InvokeMember('StringData', 'SetProperty', $null, $record, @([int]1, 'V7InstallerHelper')) | Out-Null
        $record.GetType().InvokeMember('SetStream', 'InvokeMethod', $null, $record, @([int]2, [string]$fixtureBin)) | Out-Null
        $view = $database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $database, @('INSERT INTO `Binary` (`Name`, `Data`) VALUES (?, ?)'))
        $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, @($record)) | Out-Null
        $database.GetType().InvokeMember('Commit', 'InvokeMethod', $null, $database, $null) | Out-Null
    } finally {
        if ($null -ne $record) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) | Out-Null }
        if ($null -ne $view) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) | Out-Null }
        if ($null -ne $database) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) | Out-Null }
        if ($null -ne $installer) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null }
    }

    # Invoke the exact Export-MsiBinaryStream function body from verify-v7-authenticode.ps1.
    $exportFnMatch = [regex]::Match($verifyAuthSource, '(?ms)^function Export-MsiBinaryStream\b.*?^\}')
    if (-not $exportFnMatch.Success) {
        throw 'Could not extract Export-MsiBinaryStream from verify-v7-authenticode.ps1.'
    }
    . ([scriptblock]::Create($exportFnMatch.Value))
    Export-MsiBinaryStream -MsiFile $fixtureMsi -BinaryName 'V7InstallerHelper' -DestinationPath $exportedBin
    $actualFixtureHash = (Get-FileHash -LiteralPath $exportedBin -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualFixtureHash -ne $expectedFixtureHash) {
        throw "Export-MsiBinaryStream corrupted Binary stream bytes (0x00..0xFF): expected $expectedFixtureHash, got $actualFixtureHash"
    }
} finally {
    Remove-Item -LiteralPath $msiStreamTemp -Recurse -Force -ErrorAction SilentlyContinue
}

# 3. CR-P2-15 & PRB-P1-2: Shared third-party component manifest drives both build-v7.ps1 and
#    generate_sbom.py. Always validate bundled_third_party_components() via stdlib-only Python
#    (works on CI runners without PyYAML), and additionally run full end-to-end SBOM output when PyYAML is present.
$thirdPartyPath = Join-Path $repo 'config\release\third-party-components.json'
$thirdParty = Get-Content -LiteralPath $thirdPartyPath -Raw -Encoding UTF8 | ConvertFrom-Json
$pythonCmd = Get-Command python.exe -ErrorAction SilentlyContinue
if (-not $pythonCmd) { $pythonCmd = Get-Command python -ErrorAction SilentlyContinue }
if (-not $pythonCmd) {
    throw 'Python is required to validate generate_sbom.py bundled third-party components.'
}
$sbomStdlibCheck = @"
import importlib.util, json, pathlib, sys
repo = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location('generate_sbom', repo / 'scripts' / 'release' / 'generate_sbom.py')
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
items = mod.bundled_third_party_components(repo / 'config' / 'release' / 'third-party-components.json')
print(json.dumps({'components': items}))
"@
$bundledRaw = (& $pythonCmd.Source -c $sbomStdlibCheck $repo) -join "`n"
if ($LASTEXITCODE -ne 0) {
    throw "Stdlib-only generate_sbom.bundled_third_party_components() failed with exit code $LASTEXITCODE"
}
$bundledResult = $bundledRaw | ConvertFrom-Json
$bundledByKey = @{}
foreach ($comp in @($bundledResult.components)) {
    $bundledByKey["$($comp.name)@$($comp.version)"] = $true
}
foreach ($expected in @($thirdParty.components)) {
    $key = "$($expected.name)@$($expected.version)"
    if (-not $bundledByKey.ContainsKey($key)) {
        throw "generate_sbom.bundled_third_party_components() is missing bundled component: $key"
    }
}

$previousEap = $ErrorActionPreference
$hasYaml = $false
try {
    $ErrorActionPreference = 'Continue'
    & $pythonCmd.Source -c "import yaml" 2>$null
    $hasYaml = ($LASTEXITCODE -eq 0)
} finally {
    $ErrorActionPreference = $previousEap
}
if ($hasYaml) {
    $sbomTemp = Join-Path (Join-Path $repo '.tool-cache\test-tmp') ('sbom-contract-' + [guid]::NewGuid().ToString('n') + '.json')
    try {
        & $pythonCmd.Source (Join-Path $repo 'scripts\release\generate_sbom.py') --version 10.1.0 --output $sbomTemp --ui-runtime native
        if ($LASTEXITCODE -ne 0) { throw "generate_sbom.py failed with exit code $LASTEXITCODE" }
        $sbom = Get-Content -LiteralPath $sbomTemp -Raw -Encoding UTF8 | ConvertFrom-Json
        $byName = @{}
        foreach ($comp in @($sbom.components)) {
            $byName["$($comp.name)@$($comp.version)"] = $true
        }
        foreach ($expected in @($thirdParty.components)) {
            $key = "$($expected.name)@$($expected.version)"
            if (-not $byName.ContainsKey($key)) {
                throw "Generated SBOM is missing bundled third-party component: $key"
            }
        }
    } finally {
        Remove-Item -LiteralPath $sbomTemp -Force -ErrorAction SilentlyContinue
    }
}

# 4. CR-P2-16: release-v7.yml always() cleanup must guard directories with Test-Path -PathType Container.
$releaseWorkflow = Get-Content -LiteralPath (Join-Path $repo '.github\workflows\release-v7.yml') -Raw -Encoding UTF8
if ($releaseWorkflow -notmatch 'Test-Path -LiteralPath \$dir -PathType Container') {
    throw 'release-v7.yml Stop release-owned processes step must check Test-Path -LiteralPath $dir -PathType Container before calling shutdown-running.ps1.'
}
$cleanupTemp = Join-Path (Join-Path $repo '.tool-cache\test-tmp') ('release-cleanup-' + [guid]::NewGuid().ToString('n'))
try {
    $missingDir = Join-Path $cleanupTemp 'missing-first\HLSDownloader'
    $existingDir = Join-Path $cleanupTemp 'existing-second\HLSDownloader'
    New-Item -ItemType Directory -Force -Path $existingDir | Out-Null
    $cleaned = 0
    foreach ($dir in @($missingDir, $existingDir)) {
        if (Test-Path -LiteralPath $dir -PathType Container) {
            & (Join-Path $repo 'scripts\install\shutdown-running.ps1') -InstallDir $dir -IncludeNativeHost | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "shutdown-running.ps1 failed for existing directory $dir" }
            $cleaned++
        }
    }
    if ($cleaned -ne 1) {
        throw "Expected guarded cleanup to skip missing directory and process 1 existing directory; got $cleaned"
    }
} finally {
    Remove-Item -LiteralPath $cleanupTemp -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Output 'v7 packaging, ProductCode, signing-chain, SBOM, and release-cleanup contracts passed.'
