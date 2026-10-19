$ErrorActionPreference='Stop'
$work='C:\HLSValidation'
try {
    New-Item -ItemType Directory -Force C:\HLSTests\scripts,C:\QAWork | Out-Null
    Copy-Item C:\HLSSource\scripts\validation C:\HLSTests\scripts -Recurse
    Copy-Item C:\HLSPrevious\baseline-image\HLSDownloader C:\HLSRuntime -Recurse
    Copy-Item "$work\candidate\*.exe" C:\HLSRuntime\app\resources -Force
    Copy-Item "$work\candidate\*.jar" C:\HLSRuntime\app -Force
    & "$work\resume.ps1"
} catch {$_|Out-String|Set-Content "$work\ui-only.error"}
