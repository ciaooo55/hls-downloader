$ErrorActionPreference = 'Stop'
try {
    $env:PYTHONHOME = 'C:\QAPython'
    $env:Path = 'C:\QAPython;C:\QAPython\Library\bin;C:\QAPython\DLLs;' + $env:Path
    & C:\QAInput\root-cause-verify-delivery.ps1
    & C:\QAInput\root-cause-upgrade.ps1
    & C:\QAPython\python.exe C:\QAInput\scripts\smoke_extension_media.py --browser firefox --extension C:\QAInput\root-cause-extension --browser-binary C:\QAFirefox\firefox.exe --driver C:\QAInput\geckodriver.exe --ffmpeg C:\QAInput\runtime\app\resources\ffmpeg.exe --headed --mode direct --mode multi-dash-mse --report C:\QAInput\root-cause-media-recorded.json *> C:\QAInput\root-cause-media-recorded.log
    $LASTEXITCODE | Set-Content C:\QAInput\root-cause-media-recorded.exit
    'complete' | Set-Content C:\QAInput\root-cause-package-final-done.txt
} catch { $_ | Out-String | Set-Content C:\QAInput\root-cause-package-final-error.txt }
