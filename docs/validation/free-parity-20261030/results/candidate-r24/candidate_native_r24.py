import hashlib
import zipfile
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
result = {'passed':False, 'finished':False, 'scope':'Same bafddff6 candidate; minimum 1024x600 logical window fixture with real native input at unchanged OS DPI',
          'host_product_install_performed':False, 'suites':[]}
token = 'free-parity-20261030-local-ui'

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
    expected=json.loads(Path(r'C:\QARunners\candidate-expected-r21.json').read_text(encoding='utf-8'))
    result['msi_sha256']=hashlib.sha256(msi.read_bytes()).hexdigest()
    assert result['msi_sha256']==expected['msi_sha256']
    local=work/'candidate.msi'
    local.write_bytes(msi.read_bytes())
    check=run(['msiexec.exe','/i',str(local),'/qn','INSTALLDIR='+str(install),'/norestart','/l*v',str(work/'install.log')],'install',600)
    assert check['passed'],check
    installed=True
    hashes={}
    for relative,digest in expected['critical_components'].items():
        matches=list(install.glob(relative))
        assert len(matches)==1,relative
        actual=hashlib.sha256(matches[0].read_bytes()).hexdigest()
        assert actual==digest,relative
        hashes[relative]=actual
    import zipfile
    wrappers=list((install/'app').glob('skiko-awt-runtime-windows-x64-*.jar'))
    assert len(wrappers)==1,wrappers
    with zipfile.ZipFile(wrappers[0]) as wrapper:
        assert set(wrapper.namelist())=={'META-INF/','META-INF/MANIFEST.MF'}
        assert wrapper.read('META-INF/MANIFEST.MF')==b'Manifest-Version: 1.0\r\n\r\n'
    save('skiko-wrapper-validation.json',{'installed_path':wrappers[0].name,'installed_sha256':hashlib.sha256(wrappers[0].read_bytes()).hexdigest(),'portable_wrappers':expected['stub_wrappers'],'contents':'Empty manifest only; actual native DLL verified separately'})
    save('installed-critical-components.json',hashes)
    engine=str(install/'app/resources/HLSDownloaderEngine.exe')
    host=str(install/'app/resources/HLSDownloaderNativeHost.exe')
    (work/'native-host').mkdir(exist_ok=True)
    result['source_commit']=expected['source_commit']
    save('acceptance.json',result)
    page_body=b'<html><a href="child.html">next</a><a href="index.html#top">back</a><img src="image.bin"></html>'
    class NativeHandler(RangeHandler):
        def do_HEAD(self):
            if self.path in ['/site/index.html','/site/child.html']:
                self.site_response(head=True)
            else: super().do_HEAD()
        def do_GET(self):
            if self.path.startswith('/missing-'):
                self.send_error(404,'Fixture resource is unavailable');return
            if self.path in ['/site/index.html','/site/child.html']:
                self.site_response(head=False)
            else: super().do_GET()
        def site_response(self,head):
            start,end,status=0,len(page_body)-1,200
            if not head and self.headers.get('Range','').startswith('bytes='):
                first,_,last=self.headers['Range'][6:].partition('-')
                start=int(first or 0);end=min(int(last) if last else end,end);status=206
            self.send_response(status)
            self.send_header('Content-Length',str(end-start+1))
            self.send_header('Content-Type','text/html')
            self.send_header('Accept-Ranges','bytes')
            self.send_header('ETag','"native-site"')
            if status==206:self.send_header('Content-Range',f'bytes {start}-{end}/{len(page_body)}')
            self.end_headers()
            if not head:self.wfile.write(page_body[start:end+1])
    fixture=Server(('127.0.0.1',18672),NativeHandler)
    threading.Thread(target=fixture.serve_forever,daemon=True).start()
    probes=[]
    for method,headers in [('HEAD',{}),('GET',{}),('GET',{'Range':'bytes=4-12'})]:
        with urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:18672/site/index.html',method=method,headers=headers)) as response:
            body=response.read()
            probes.append({'method':method,'request_headers':headers,'status':response.status,'content_length':int(response.headers['Content-Length']),'content_range':response.headers.get('Content-Range'),'received_bytes':len(body)})
            assert int(response.headers['Content-Length'])==(9 if headers else len(page_body))
            assert body==(b'' if method=='HEAD' else page_body[4:13] if headers else page_body)
    save('fixture-http-contract.json',{'passed':True,'probes':probes})
    os.environ.update(HLS_V7_DATA_DIR=str(work/'native/data'),HLS_V7_DOWNLOAD_DIR=str(work/'native/downloads'),
                      HLS_V7_CORE_TCP='1',HLS_V7_CORE_BIND='127.0.0.1:18679',
                      HLS_UI_TEST_API='1',HLS_UI_TEST_TOKEN=token,HLS_UI_TEST_PORT='18744',HLS_UI_AUDIT_WIDTH='1024',HLS_UI_AUDIT_HEIGHT='600')
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
            'url':f'http://127.0.0.1:18672/seed-{index}.bin','filename':('长文件名'+str(index)+'-')*8+'.bin',
            'resource_kind':'file','title':('长标题与圆角裁切检查 '+str(index)+' ')*16,'scheduled_start_at':'2050-01-01T00:00:00Z'}}})
    send(ipc,{'type':'command','request_id':4,'command':{'kind':'create_task','spec':{
        'url':'http://127.0.0.1:18672/missing-'+('long-path-'*20)+'.bin','filename':'error-fixture.bin',
        'title':'错误与长文本检查：'+('服务器没有该资源；请核对地址。'*16),'resource_kind':'file'}}})
    # Protocol-only completed output fixture for uninstall preservation; not a native create claim.
    output_created=send(ipc,{'type':'command','request_id':3,'command':{'kind':'create_task','spec':{
        'url':'http://127.0.0.1:18672/output-fixture.bin','filename':'output-fixture.bin','resource_kind':'file'}}})
    save('output-fixture-create.json',output_created)
    output_id=next(e['event']['snapshot']['task_id'] for e in output_created['events'] if e['event']['kind']=='task_created')
    output_task=wait_task(ipc,output_id,lambda t:t['status'] in ('completed','failed'),100)
    assert output_task['status']=='completed',output_task
    assert hashlib.sha256(Path(output_task['output_path']).read_bytes()).hexdigest()==hashlib.sha256(BLOCK*(PAYLOAD_SIZE//len(BLOCK))).hexdigest()
    save('output-fixture-completed.json',output_task)
    os.environ.pop('JAVA_TOOL_OPTIONS',None)
    (work/'native/inputs').mkdir(parents=True,exist_ok=True)
    (work/'native/inputs/tasks.txt').write_text('http://127.0.0.1:18672/import-one.bin\nhttp://127.0.0.1:18672/import-two.bin\n')
    save('native-fixtures.json',{'url':'http://127.0.0.1:18672/native.bin',
        'batch_urls':['http://127.0.0.1:18672/batch-one.bin','http://127.0.0.1:18672/batch-two.bin'],
        'import_file':str(work/'native/inputs/tasks.txt'),
        'output_dir':str(work/'native/downloads'),
        'expected_bytes':PAYLOAD_SIZE,'expected_sha256':hashlib.sha256(BLOCK*(PAYLOAD_SIZE//len(BLOCK))).hexdigest()})
    with zipfile.ZipFile(work/'native/inputs/预览.zip','w') as archive:
        archive.writestr('中文目录/说明.txt','免费版目录预览')
        for i in range(120): archive.writestr(f'long-path-{i}/example.txt','sample')
    cards=work/'native/inputs/native-input-cards.txt'
    cards.write_text('http://127.0.0.1:18672/native.bin\nhttp://127.0.0.1:18672/batch-one.bin\nhttp://127.0.0.1:18672/batch-two.bin\nhttp://127.0.0.1:18672/site/index.html\nC:\\QAWork\\native\\inputs\\预览.zip\ncurl http://127.0.0.1:18672/curl.bin \\\n  -H X-Native-Card:yes\n',encoding='utf-8')
    # A plain text fixture allows real in-Sandbox copy/paste without relying on host RDP clipboard propagation.
    subprocess.Popen(['notepad.exe',str(cards)],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    gui=subprocess.Popen([str(install/'HLSDownloader.exe')],cwd=install)
    result['phase']='native-input-ready'
    save('acceptance.json',result)
    deadline=time.monotonic()+10800
    restart_count=0
    while not (report/'native-finished.json').is_file() and time.monotonic()<deadline:
        try:
            request=urllib.request.Request('http://127.0.0.1:18744/state',headers={'X-HLS-Test-Token':token})
            with urllib.request.urlopen(request,timeout=2) as response:
                save('state-latest.json',json.load(response))
            window_request=urllib.request.Request('http://127.0.0.1:18744/window',headers={'X-HLS-Test-Token':token})
            with urllib.request.urlopen(window_request,timeout=2) as response:
                save('window-latest.json',json.load(response))
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
    core_log.close()
    result['comparison']={'executed':False,'reason':'UI workbench inspection; no performance comparison in this run'}
    item=run(['msiexec.exe','/x',str(local),'/qn','/norestart','/l*v',str(work/'uninstall.log')],'uninstall',600)
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
        result['cleanup_uninstall']=run(['msiexec.exe','/x',str(local),'/qn','/norestart','/l*v',str(work/'cleanup-uninstall.log')],'cleanup-uninstall',600)
    import shutil
    for log in work.glob('*.log'): shutil.copyfile(log,report/log.name)
    collect_diagnostics()
    if fixture: fixture.shutdown();fixture.server_close()
    result['finished']=True
    save('acceptance.json',result)
