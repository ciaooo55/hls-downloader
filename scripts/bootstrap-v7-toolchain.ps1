[CmdletBinding()]
param([string]$JdkRoot='E:\HLSDownloaderBuildCache\jdk-21',[string]$GradleHome='E:\HLSDownloaderBuildCache\gradle')
$ErrorActionPreference='Stop'
if(Test-Path "$JdkRoot\bin\java.exe"){ Write-Output "JDK already ready: $JdkRoot"; exit 0 }
New-Item -ItemType Directory -Force -Path $GradleHome,(Split-Path $JdkRoot -Parent) | Out-Null
$zip=Join-Path $env:TEMP 'hlsdownloader-temurin21.zip'
Invoke-WebRequest -Uri 'https://api.adoptium.net/v3/binary/latest/21/ga/windows/x64/jdk/hotspot/normal/eclipse' -OutFile $zip
$unpack=Join-Path $env:TEMP 'hlsdownloader-temurin21'
Remove-Item -LiteralPath $unpack -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive -LiteralPath $zip -DestinationPath $unpack -Force
$source=Get-ChildItem -LiteralPath $unpack -Directory | Select-Object -First 1
Move-Item -LiteralPath $source.FullName -Destination $JdkRoot -Force
Remove-Item -LiteralPath $zip,$unpack -Recurse -Force -ErrorAction SilentlyContinue
Write-Output "JDK installed: $JdkRoot"