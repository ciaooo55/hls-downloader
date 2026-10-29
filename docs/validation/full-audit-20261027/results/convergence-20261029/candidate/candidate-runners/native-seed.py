import json,os,pathlib,subprocess,sys,time
sys.path.insert(0,r'C:\QAScripts')
from msi_checkpoint_fixture import connect,send
assert os.environ.get('USERNAME')=='WDAGUtilityAccount'
env=os.environ.copy();env.update(HLS_V7_CORE_TCP='1',HLS_V7_CORE_BIND='127.0.0.1:18679',HLS_V7_PIPE=r'\\.\pipe\HLSDownloader.candidate-seed')
p=subprocess.Popen([r'C:\QACandidateInstalled\app\resources\HLSDownloaderEngine.exe'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
try:
 s=connect(18679)
 send(s,{'type':'store_settings','request_id':1,'values':{'legal_terms_accepted':True,'download_progress_window_enabled':False,'download_complete_popup_enabled':False,'av_scan_enabled':False}})
 for n in range(1,4):
  r=send(s,{'type':'command','request_id':10+n,'command':{'kind':'create_task','spec':{'url':f'https://fixture.invalid/selection-{n}.bin','resource_kind':'file','title':f'Sandbox selection fixture {n}','filename':f'selection-{n}.bin','scheduled_start_at':'2050-01-01T00:00:00Z'}}})
  assert r['type']!='error',r
 r=send(s,{'type':'snapshot','request_id':20}); assert len(r['tasks'])==4,r
 pathlib.Path(r'C:\QACandidateReports\native-seed.json').write_text(json.dumps(r,indent=2),encoding='utf-8')
 for action in ('accept','reject'):
  key='native-ui-'+action
  handoff={'id':key,'request_id':key,'offer':{'handoff_id':key,'url':'data:application/octet-stream,','resource_kind':'file','local_source_path':r'C:\QACandidateWork\checkpoint\downloads\fixture.bin','filename':'ui-'+action+'.bin','title':'Sandbox native confirmation '+action,'mime_type':'application/octet-stream','size':33554432},'filename':'ui-'+action+'.bin','title':'Sandbox native confirmation '+action,'mime_type':'application/octet-stream','size':33554432,'status':'pending','presentation':'queued','task_id':None,'created_at_ms':int(time.time()*1000)}
  reply=send(s,{'type':'admit_handoff','request_id':30,'handoff_json':json.dumps(handoff)})
  assert reply['type']=='handoffs',reply

finally:
 p.terminate();p.wait(timeout=10)
