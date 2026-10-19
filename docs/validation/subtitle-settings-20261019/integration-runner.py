import json,subprocess,time,traceback
from pathlib import Path
from smoke_v7_portable_app import Pipe
core=subprocess.Popen(['C:/QAWork/Custom Install/HLSDownloader/app/resources/HLSDownloaderEngine.exe'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,stdin=subprocess.DEVNULL,creationflags=subprocess.CREATE_NO_WINDOW)
try:
    for attempt in range(100):
        try:
            p=Pipe(); break
        except OSError: time.sleep(.1)
    else: raise RuntimeError('Core did not start')
    try:
        preferences=json.loads(Path('C:/HLSQA/configuration-saved.json').read_text(encoding='utf-8'))['preferences']
        result=p.request({'type':'subtitle','request_id':1,'request':{'action':'configure','provider':'qwencloud','api_key':'not-a-real-key-subtitle-config-sandbox','preferences':preferences}})
        assert result['result']['ok'],result
    finally:p.close()
    with open('C:/HLSQA/real-core-run.log','w',encoding='utf-8') as log:
        r=subprocess.run(['C:/HLSQA/python/python.exe','-u','C:/HLSQA/real-core-browser.py'],stdout=log,stderr=subprocess.STDOUT,timeout=110)
        assert r.returncode==0,r.returncode
except BaseException:
    Path('C:/HLSQA/integration-failure.txt').write_text(traceback.format_exc(),encoding='utf-8')
    raise
finally:
    subprocess.run(['taskkill.exe','/F','/T','/PID',str(core.pid)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
