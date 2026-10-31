import json,os,subprocess,sys,traceback
from pathlib import Path
assert os.environ.get('USERNAME')=='WDAGUtilityAccount'
os.environ.update(PYTHONPATH=r'C:\QADeps;C:\QAScripts',PYTHONIOENCODING='utf-8',PYTHONDONTWRITEBYTECODE='1')
report=Path(r'C:\QAReports');results=[]
import time
start_limit=time.monotonic()+600
while not (report/'start-acceptance.json').is_file() and time.monotonic()<start_limit:time.sleep(1)
assert (report/'start-acceptance.json').is_file(),'native desktop preparation timeout'
for name in ['candidate_native_r24']:
 try:
  with (report/(name+'.stdout.txt')).open('wb') as out,(report/(name+'.stderr.txt')).open('wb') as err:
   child=subprocess.Popen([sys.executable,str(Path(r'C:\QARunners')/(name+'.py'))],stdin=subprocess.DEVNULL,stdout=out,stderr=err,creationflags=subprocess.CREATE_NO_WINDOW)
   code=child.wait(timeout=14400)
  results.append({'name':name,'exit_code':code})
 except Exception as error:results.append({'name':name,'error':repr(error),'traceback':traceback.format_exc()})
 (report/'candidate-exits.json').write_text(json.dumps(results,indent=2),encoding='utf-8')
