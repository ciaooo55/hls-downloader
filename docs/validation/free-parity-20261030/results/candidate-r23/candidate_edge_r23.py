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
result = {'passed':False, 'finished':False, 'scope':'Same bafddff6 candidate; Edge reload fixture diagnosis and targeted protocol followup; no desktop input',
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
    fixtures=run([sys.executable,r'C:\QARunners\media_guest.py','--resources',str(install/'app/resources'),'--work',str(work/'media'),'--report',str(report/'media-fixtures.json'),'--fixtures-only'],'media-fixtures',120)
    assert fixtures['passed'],fixtures
    stop_installed()
    import zipfile
    for family,folder in [('Chromium','chrome-mv3'),('Firefox','firefox-mv3')]:
        with zipfile.ZipFile(install/f'app/resources/extensions/HLSDownloader-7.0.3-{family}.zip') as archive:
            archive.extractall(Path(r'C:\QACurrentExtensions')/folder)
    Path(r'C:\QACurrentWork').mkdir(exist_ok=True)
    os.environ.update(HLS_CURRENT_RESOURCES=str(install/'app/resources'),HLS_REPLY_REAL_HOST=host,
        HLS_REPLY_PROXY_PATH=str(work/'NativeReplyProxy.exe'),HLS_BROWSER_HEADFUL='1')
    compile_proxy=run(['powershell.exe','-NoProfile','-Command',r'Add-Type -Path C:\QARunners\NativeReplyProxy.cs -ReferencedAssemblies System.Web.Extensions,System.Core -OutputAssembly C:\QAWork\NativeReplyProxy.exe -OutputType ConsoleApplication'],'compile-proxy',120)
    assert compile_proxy['passed'],compile_proxy
    for family in ['edge']:
        os.environ.update(HLS_V7_DATA_DIR=str(work/f'browser-{family}-data'),HLS_V7_DOWNLOAD_DIR=str(work/f'browser-{family}-downloads'),
            HLS_V7_PIPE=rf'\\.\pipe\HLSDownloader.free-{family}',HLS_CURRENT_REPORT_DIR=str(report/family),
            HLS_REPLY_PROXY_STATE=str(report/family/'proxy'),HLS_REPLY_FAILURE_COUNT='1')
        (report/family/'proxy').mkdir(parents=True,exist_ok=True)
        arguments=[r'C:\QANode\node.exe',r'C:\QARunners\current-browser-reload-r22.mjs']+(['--firefox'] if family=='firefox' else [])
        result['suites'].append(run(arguments,f'{family}-matrix',420))
    os.environ.pop('HLS_V7_PIPE',None)
    stop_installed()
    result['passed']=all(item['passed'] for item in result['suites'])
except Exception as error:
    import traceback
    result.update(error=repr(error),traceback=traceback.format_exc())
finally:
    stop_installed()
    if installed:
        item=run(['msiexec.exe','/x',str(local),'/qn','/norestart','/l*v',str(work/'uninstall.log')],'uninstall',600)
        result['uninstall']=item
        result['passed']=result['passed'] and item['passed']
        if (work/'uninstall.log').exists(): (report/'uninstall.log').write_bytes((work/'uninstall.log').read_bytes())
    collect_diagnostics()
    result['finished']=True
    save('acceptance.json',result)
    save('guest-exit.json',{'exit_code':0 if result['passed'] else 1})
sys.exit(0 if result['passed'] else 1)
