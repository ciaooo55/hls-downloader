. C:\QARunners\common.ps1
Copy-Item C:\QAInput\final-tests.exe,C:\QAPerf\throttle\vcruntime140.dll C:\QAWork
Copy-Item C:\QAPerf\runtime\app\resources\app-icon.ico C:\QAWork
Run C:\QAWork\final-tests.exe @('media::merge::tests::','--include-ignored','--test-threads=1') 'merge-regression'
Run C:\QAWork\final-tests.exe @('http_engine::tests::curl_','--ignored','--test-threads=1') 'curl-regression'
Run C:\QAWork\final-tests.exe @('http_engine::tests::winhttp_','--ignored','--test-threads=1') 'winhttp-regression'
