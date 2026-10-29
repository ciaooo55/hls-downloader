import json,os,pathlib,sys,time
sys.path.insert(0,r'C:\QAScripts')
from msi_checkpoint_fixture import connect,send
assert os.environ.get('USERNAME')=='WDAGUtilityAccount'
s=connect(18679)
r={'snapshot':send(s,{'type':'snapshot','request_id':1}),'handoffs':send(s,{'type':'load_handoffs','request_id':2})}
r['source_exists']=pathlib.Path(r'C:\QACandidateWork\checkpoint\downloads\fixture.bin').is_file()
pathlib.Path(r'C:\QACandidateReports\native-final-core.json').write_text(json.dumps(r,indent=2),encoding='utf-8')

import urllib.request,subprocess
headers={'X-HLS-Test-Token':'candidate-local-20261029-ui'}
def read_api(endpoint):
    with urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:18744/'+endpoint,headers=headers),timeout=2) as response:return json.load(response)
report=pathlib.Path(r'C:\QACandidateReports')
(report/'native-window-pinned.json').write_text(json.dumps(read_api('window'),indent=2),encoding='utf-8')
# Restart only the sandbox-installed JVM launcher, preserving the running Core and data.
subprocess.run(['powershell.exe','-NoProfile','-Command',r"Get-CimInstance Win32_Process | Where-Object {$_.ExecutablePath -eq 'C:\QACandidateInstalled\HLSDownloader.exe'} | ForEach-Object {Stop-Process -Id $_.ProcessId -Force}"],check=True)
env=os.environ.copy()
for key in ('HLS_UI_AUDIT_WIDTH','HLS_UI_AUDIT_HEIGHT'):env.pop(key,None)
gui=subprocess.Popen([r'C:\QACandidateInstalled\HLSDownloader.exe'],cwd=r'C:\QACandidateInstalled',env=env)
deadline=time.monotonic()+30
while time.monotonic()<deadline:
    try:
        metadata=read_api('window');break
    except OSError:time.sleep(.2)
else:raise RuntimeError('Normal-geometry UI did not become ready')
(report/'native-window-normal.json').write_text(json.dumps(metadata,indent=2),encoding='utf-8')
(report/'native-normal-geometry-ready.json').write_text(json.dumps({'ready':True,'scope':'Same installed candidate; only audit width/height removed; running Core preserved'}),encoding='utf-8')
deadline=time.monotonic()+1200
while not (report/'native-settings-finished.json').is_file() and time.monotonic()<deadline:
    try:(report/'native-state-latest.json').write_text(json.dumps(read_api('state'),indent=2),encoding='utf-8')
    except OSError:pass
    time.sleep(2)
assert (report/'native-settings-finished.json').is_file(),'Settings input handoff timeout'
assert json.loads((report/'native-settings-finished.json').read_text(encoding='utf-8-sig'))['passed']
