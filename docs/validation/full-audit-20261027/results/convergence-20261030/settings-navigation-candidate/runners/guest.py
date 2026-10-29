import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
import urllib.request

assert os.environ.get('USERNAME') == 'WDAGUtilityAccount', 'Windows Sandbox only'
sys.path[:0] = [r'C:\QAScripts', r'C:\QADeps']
import psutil
from msi_checkpoint_fixture import BLOCK, PAYLOAD_SIZE, RangeHandler, Server, connect, send, wait_task

report = Path(r'C:\QAReports')
work = Path(r'C:\QAWork')
install = Path(r'C:\QAInstalled')
work.mkdir(exist_ok=True)
os.chdir(work)
os.environ.update(HLS_VM_ACCEPTANCE='1', PYTHONPATH=r'C:\QADeps;C:\QAScripts',
                  TMP=str(work/'tmp'), TEMP=str(work/'tmp'), PYTHONUTF8='1')
(work/'tmp').mkdir(exist_ok=True)
result = {'passed':False, 'finished':False, 'scope':'Settings navigation and workbench visual acceptance',
          'host_product_install_performed':False, 'suites':[]}
token = 'visual-controls-20261029-local-ui'

def save(name, value):
    (report/name).write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding='utf-8')

def run(arguments, name, timeout):
    result['phase'] = name
    save('acceptance.json', result)
    with (report/(name+'.stdout.txt')).open('wb') as out, (report/(name+'.stderr.txt')).open('wb') as err:
        proc = subprocess.Popen(arguments, stdout=out, stderr=err)
        try:
            code = proc.wait(timeout=timeout)
            return {'name':name, 'exit_code':code, 'passed':code==0}
        except subprocess.TimeoutExpired:
            parent = psutil.Process(proc.pid)
            descendants = parent.children(recursive=True)
            for child in reversed(descendants):
                try: child.terminate()
                except psutil.NoSuchProcess: pass
            proc.terminate()
            psutil.wait_procs(descendants, timeout=10)
            proc.wait(timeout=15)
            return {'name':name, 'passed':False, 'error':'runner timeout', 'timeout_seconds':timeout}

def collect_diagnostics():
    diagnostics=report/'installer-diagnostics'
    diagnostics.mkdir(exist_ok=True)
    for directory in [work/'tmp',Path(r'C:\Windows\Temp'),Path(os.environ.get('LOCALAPPDATA','C:/QAWork'))/'Temp']:
        for path in directory.glob('HLSInstaller-*.log'):
            try: (diagnostics/(directory.name+'-'+path.name)).write_bytes(path.read_bytes())
            except OSError: pass

def stop_installed():
    owned=[]
    for proc in psutil.process_iter(['exe']):
        try:
            if proc.info['exe'] and Path(proc.info['exe']).is_relative_to(install):
                proc.terminate()
                owned.append(proc)
        except psutil.NoSuchProcess: pass
    _, alive=psutil.wait_procs(owned,timeout=15)
    for proc in alive: proc.kill()

installed=False
fixture=None
try:
    msi=Path(r'C:\QACandidate\HLSDownloader-7.0.3-Windows-x64-candidate.msi')
    expected=json.loads(Path(r'C:\QARunners\expected.json').read_text(encoding='utf-8'))
    result['msi_sha256']=hashlib.sha256(msi.read_bytes()).hexdigest()
    assert result['msi_sha256']==expected['msi_sha256']
    local=work/'candidate.msi'
    local.write_bytes(msi.read_bytes())
    check=run(['msiexec.exe','/i',str(local),'/qn','INSTALLDIR='+str(install),'/norestart','/l*v',str(report/'install.log')],'install',180)
    assert check['passed'],check
    installed=True
    hashes={}
    for relative,digest in expected['critical_components'].items():
        matches=list(install.glob(relative))
        assert len(matches)==1,relative
        actual=hashlib.sha256(matches[0].read_bytes()).hexdigest()
        assert actual==digest,relative
        hashes[relative]=actual
    save('installed-critical-components.json',hashes)
    engine=str(install/'app/resources/HLSDownloaderEngine.exe')
    host=str(install/'app/resources/HLSDownloaderNativeHost.exe')
    (work/'native-host').mkdir(exist_ok=True)
    suites=[]
    for name,args,timeout in suites:
        args[0]=str(Path(r'C:\QAScripts')/args[0])
        item=run([sys.executable,*args,'--report',str(report/(name+'.json'))],name,timeout)
        result['suites'].append(item)
        save('acceptance.json',result)
    stop_installed()
    fixture=Server(('127.0.0.1',18672),RangeHandler)
    threading.Thread(target=fixture.serve_forever,daemon=True).start()
    os.environ.update(HLS_V7_DATA_DIR=str(work/'native/data'),HLS_V7_DOWNLOAD_DIR=str(work/'native/downloads'),
                      HLS_V7_CORE_TCP='1',HLS_V7_CORE_BIND='127.0.0.1:18679',
                      HLS_UI_TEST_API='1',HLS_UI_TEST_TOKEN=token,HLS_UI_TEST_PORT='18744')
    core_log=(report/'native-core.log').open('wb')
    core=subprocess.Popen([engine],stdout=core_log,stderr=core_log)
    ipc=connect(18679)
    queue_profiles=[{'id':'default','name':'默认队列','enabled':True,'priority':0,'max_active':1,'speed_limit_kib':0,'schedule_enabled':False,'start_time':'00:00','stop_time':'23:59','active_days':'1,2,3,4,5,6,7','completion_action':'none'}]
    queue_profiles += [dict(queue_profiles[0],id=f'visual-{i}',name=f'测试队列 {i:02}') for i in range(1,13)]
    native_setup=send(ipc,{'type':'store_settings','request_id':1,'values':{
        'legal_terms_accepted':True,'legal_terms_version':'2026-08-06-cn-1',
        'download_progress_window_enabled':False,'download_complete_popup_enabled':False,
        'default_concurrency':1,'queue_max_active':1,'queue_profiles':queue_profiles}})
    assert native_setup['type']=='settings', native_setup
    assert native_setup['default_concurrency']==1 and not native_setup['progress_window_enabled'] and not native_setup['complete_popup_enabled'], native_setup
    save('native-fixture-settings.json',native_setup)
    # Setup fixtures is protocol-only; every later UI action uses native input.
    for index in range(1,4):
        send(ipc,{'type':'command','request_id':2,'command':{'kind':'create_task','spec':{
            'url':f'http://127.0.0.1:18672/seed-{index}.bin','filename':f'seed-{index}.bin',
            'resource_kind':'file','scheduled_start_at':'2050-01-01T00:00:00Z'}}})
    # Protocol-only completed output fixture for uninstall preservation; not a native create claim.
    output_created=send(ipc,{'type':'command','request_id':3,'command':{'kind':'create_task','spec':{
        'url':'http://127.0.0.1:18672/output-fixture.bin','filename':'output-fixture.bin','resource_kind':'file'}}})
    save('output-fixture-create.json',output_created)
    output_id=next(e['event']['snapshot']['task_id'] for e in output_created['events'] if e['event']['kind']=='task_created')
    output_task=wait_task(ipc,output_id,lambda t:t['status'] in ('completed','failed'),100)
    assert output_task['status']=='completed',output_task
    assert hashlib.sha256(Path(output_task['output_path']).read_bytes()).hexdigest()==hashlib.sha256(BLOCK*(PAYLOAD_SIZE//len(BLOCK))).hexdigest()
    save('output-fixture-completed.json',output_task)
    os.environ['JAVA_TOOL_OPTIONS']='-Dsun.java2d.uiScale=1.5'
    (work/'native/inputs').mkdir(parents=True,exist_ok=True)
    (work/'native/inputs/tasks.txt').write_text('http://127.0.0.1:18672/import-one.bin\nhttp://127.0.0.1:18672/import-two.bin\n')
    save('native-fixtures.json',{'url':'http://127.0.0.1:18672/native.bin',
        'batch_urls':['http://127.0.0.1:18672/batch-one.bin','http://127.0.0.1:18672/batch-two.bin'],
        'import_file':str(work/'native/inputs/tasks.txt'),
        'output_dir':str(work/'native/downloads'),
        'expected_bytes':PAYLOAD_SIZE,'expected_sha256':hashlib.sha256(BLOCK*(PAYLOAD_SIZE//len(BLOCK))).hexdigest()})
    gui=subprocess.Popen([str(install/'HLSDownloader.exe')],cwd=install)
    result['phase']='native-input-ready'
    save('acceptance.json',result)
    deadline=time.monotonic()+3600
    restart_count=0
    while not (report/'native-finished.json').is_file() and time.monotonic()<deadline:
        try:
            request=urllib.request.Request('http://127.0.0.1:18744/state',headers={'X-HLS-Test-Token':token})
            with urllib.request.urlopen(request,timeout=2) as response:
                save('state-latest.json',json.load(response))
            save('settings-latest.json',send(ipc,{'type':'load_settings','request_id':20}))
            save('core-latest.json',send(ipc,{'type':'snapshot','request_id':21}))
            marker=report/f'restart-ui-{restart_count+1}.json'
            if marker.is_file():
                config=json.loads(marker.read_text(encoding='utf-8-sig'))
                gui.terminate();gui.wait(timeout=15)
                launch_env=dict(os.environ)
                if config.get('java_scale'):
                    launch_env['JAVA_TOOL_OPTIONS']=f"-Dsun.java2d.uiScale={config['java_scale']}"
                gui=subprocess.Popen([str(install/'HLSDownloader.exe')],cwd=install,env=launch_env)
                restart_count+=1
                save(f'restart-completed-{restart_count}.json',config)
        except OSError: pass
        time.sleep(2)
    assert (report/'native-finished.json').is_file(),'native-input deadline'
    result['native']=json.loads((report/'native-finished.json').read_text(encoding='utf-8-sig'))
    snapshots=send(ipc,{'type':'snapshot','request_id':22})
    save('core-final.json',snapshots)
    save('settings-final.json',send(ipc,{'type':'load_settings','request_id':23}))
    outputs={t['output_path']:hashlib.sha256(Path(t['output_path']).read_bytes()).hexdigest()
             for t in snapshots['tasks'] if t['status']=='completed' and Path(t['output_path']).is_file()}
    ipc.close()
    stop_installed()
    core_log.close()
    result['comparison']={'executed':False,'reason':'UI workbench inspection; no performance comparison in this run'}
    item=run(['msiexec.exe','/x',str(local),'/qn','/norestart','/l*v',str(report/'uninstall.log')],'uninstall',180)
    collect_diagnostics()
    result['uninstall']=item
    save('output-hashes.json',outputs)
    result['uninstall_preserves_outputs']=bool(outputs) and all(Path(path).is_file() and hashlib.sha256(Path(path).read_bytes()).hexdigest()==digest for path,digest in outputs.items())
    assert item['passed'],item
    installed=False
    result['uninstall_preserves_outputs']=bool(outputs) and all(Path(path).is_file() and hashlib.sha256(Path(path).read_bytes()).hexdigest()==digest for path,digest in outputs.items())
    save('output-hashes.json',outputs)
    result['passed']=all(x['passed'] for x in result['suites']) and result['native']['passed'] and result['uninstall_preserves_outputs']
except Exception as error:
    import traceback
    result['error']=repr(error)
    result['traceback']=traceback.format_exc()
finally:
    if installed:
        stop_installed()
        result['cleanup_uninstall']=run(['msiexec.exe','/x',str(local),'/qn','/norestart','/l*v',str(report/'cleanup-uninstall.log')],'cleanup-uninstall',180)
    collect_diagnostics()
    if fixture: fixture.shutdown();fixture.server_close()
    result['finished']=True
    save('acceptance.json',result)
