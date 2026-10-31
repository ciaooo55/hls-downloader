import ctypes,hashlib,json,os,shutil,subprocess,sys,time,traceback,zipfile
from pathlib import Path
assert os.environ.get('USERNAME')=='WDAGUtilityAccount','Windows Sandbox only'
sys.path[:0]=[r'C:\QADeps',r'C:\QAScripts']
import psutil
report=Path(r'C:\QAReports\lifecycle');report.mkdir(exist_ok=True)
work=Path(r'C:\QALifecycle');work.mkdir(exist_ok=True)
install=work/'installed';candidate=work/'candidate.msi';baseline=work/'baseline.msi'
shutil.copyfile(r'C:\QACandidate\HLSDownloader-7.0.3-Windows-x64-candidate.msi',candidate)
shutil.copyfile(r'C:\QAPackages\HLSDownloader-7.0.3-f53019e.msi',baseline)
os.chdir(work)
os.environ.update(HLS_VM_ACCEPTANCE='1',HLS_V6_SKIP_MIGRATE='1',HLS_V7_DATA_DIR=str(work/'checkpoint/data'),HLS_V7_DOWNLOAD_DIR=str(work/'checkpoint/downloads'))
for k in ['HLS_V7_PIPE','HLS_V7_CORE_BIND']:os.environ.pop(k,None)
result={'passed':False,'finished':False,'steps':[],'scope':'Same-version production MSI upgrade, forced failure rollback, checkpoint recovery and idle uninstall; portable tested separately'}
msiapi=ctypes.WinDLL('msi');msiapi.MsiQueryProductStateW.argtypes=[ctypes.c_wchar_p];msiapi.MsiQueryProductStateW.restype=ctypes.c_int
installed=None

def digest(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def save(): (report/'lifecycle.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
def run(args,name,expected=0,timeout=600):
    result['phase']=name;save();start=time.monotonic()
    with (report/(name+'.stdout.txt')).open('wb') as out,(report/(name+'.stderr.txt')).open('wb') as err:
        p=subprocess.Popen([str(a) for a in args],stdin=subprocess.DEVNULL,stdout=out,stderr=err,creationflags=subprocess.CREATE_NO_WINDOW)
        try:code=p.wait(timeout=timeout)
        except subprocess.TimeoutExpired:p.kill();p.wait();raise RuntimeError(name+' timeout')
    result['steps'].append({'name':name,'exit_code':code,'expected':expected,'seconds':time.monotonic()-start});save()
    assert code==expected,(name,code,expected)
    return (report/(name+'.stdout.txt')).read_text(encoding='utf-8-sig',errors='replace')
def msi(source,action,name,expected=0):
    log=work/(name+'.log')
    try:return run(['msiexec.exe',action,source,'/qn','/norestart','REBOOT=ReallySuppress','INSTALLDIR='+str(install),'/l*v',log],name,expected)
    finally:
        if log.exists():shutil.copyfile(log,report/log.name)
def props(source,name,mode='inspect'):
    return json.loads(run(['powershell.exe','-NoProfile','-ExecutionPolicy','Bypass','-File',r'C:\QARunners\msi-tools.ps1','-Mode',mode,'-Path',source],name))
def checkpoint(mode,name):
    run([sys.executable,r'C:\QAScripts\msi_checkpoint_fixture.py','--mode',mode,'--engine',install/'app/resources/HLSDownloaderEngine.exe','--root',work/'checkpoint','--state',work/'checkpoint/state.json','--core-port','18671','--origin-port','18672','--report',report/(name+'.json')],name)
    return json.loads((report/(name+'.json')).read_text())
def processes():
    found=[]
    for p in psutil.process_iter(['exe']):
        try:
            if p.info['exe'] and Path(p.info['exe']).is_relative_to(install):found.append(p)
        except psutil.Error:pass
    return found
def start_running(name):
    p=subprocess.Popen([str(install/'app/resources/HLSDownloaderEngine.exe')],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    g=subprocess.Popen([str(install/'HLSDownloader.exe')],cwd=install,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    time.sleep(8)
    assert p.poll() is None and g.poll() is None,(name,p.poll(),g.poll())
    result[name]={'core_pid':p.pid,'gui_pid':g.pid};save()
    return [p,g]
try:
    old=props(baseline,'baseline-properties');new=props(candidate,'candidate-properties')
    result['baseline']=dict(old,sha256=digest(baseline));result['candidate']=dict(new,sha256=digest(candidate));save()
    assert old['UpgradeCode']==new['UpgradeCode'] and old['ProductCode']!=new['ProductCode']
    assert msiapi.MsiQueryProductStateW(old['ProductCode'])==-1 and msiapi.MsiQueryProductStateW(new['ProductCode'])==-1
    msi(baseline,'/i','baseline-install');installed=old['ProductCode']
    before=checkpoint('create-pause','baseline-checkpoint')
    old_engine=digest(install/'app/resources/HLSDownloaderEngine.exe')
    original_hash=digest(candidate);forced=work/'forced-rollback.msi';shutil.copyfile(candidate,forced)
    props(forced,'inject-deferred-failure','inject')
    msi(forced,'/i','forced-rollback',1603)
    assert digest(candidate)==original_hash
    assert msiapi.MsiQueryProductStateW(old['ProductCode'])==5 and msiapi.MsiQueryProductStateW(new['ProductCode'])!=5
    assert digest(install/'app/resources/HLSDownloaderEngine.exe')==old_engine
    rollback=checkpoint('verify-paused','rollback-task-retained')
    assert rollback['task_id']==before['task_id'] and rollback['downloaded_bytes']>=before['downloaded_bytes']
    running=start_running('before-running-upgrade')
    msi(candidate,'/i','running-cover-upgrade');installed=new['ProductCode']
    assert all(p.poll() is not None for p in running),'old processes survived upgrade'
    assert msiapi.MsiQueryProductStateW(new['ProductCode'])==5 and msiapi.MsiQueryProductStateW(old['ProductCode'])==-1
    expected=json.loads(Path(r'C:\QARunners\candidate-expected-r21.json').read_text())
    hashes={}
    for relative,sha in expected['critical_components'].items():
        paths=list(install.glob(relative));assert len(paths)==1,relative
        assert digest(paths[0])==sha,relative
        hashes[relative]=sha
    result['upgraded_components']=hashes;save()
    after=checkpoint('verify-resume','upgrade-task-resumed')
    assert after['task_id']==before['task_id'] and after['file_sha256']==after['expected_sha256']
    db=Path(after['data_db']);dbsha=digest(db);output=work/'checkpoint/downloads/fixture.bin';outsha=digest(output)
    assert not processes(),'product should be idle after checkpoint runner'
    msi(candidate,'/x','idle-uninstall');installed=None
    assert msiapi.MsiQueryProductStateW(new['ProductCode'])==-1
    assert digest(db)==dbsha and digest(output)==outsha and not (install/'HLSDownloader.exe').exists()
    result['uninstall_preserved_database_and_output']=True;save()
    result['passed']=True
except Exception as e:result.update(error=repr(e),traceback=traceback.format_exc())
finally:
    if installed:
        try:
            for p in processes():
                try:p.terminate()
                except psutil.Error:pass
            msi(installed,'/x','cleanup-uninstall')
        except Exception as e:result['cleanup_error']=repr(e)
    result['finished']=True;save()
