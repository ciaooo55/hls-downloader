import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import urllib.request

assert os.environ.get('USERNAME') == 'WDAGUtilityAccount', 'Windows Sandbox only'
sys.path.insert(0, r'C:\QAScripts')
from msi_checkpoint_fixture import connect, send

report = Path(r'C:\QACandidateReports')
work = Path(r'C:\QACandidateWork')
install = Path(r'C:\QACandidateInstalled')
work.mkdir(exist_ok=True)
result = {'passed': False, 'finished': False, 'scope': 'Remaining native UI and fixed-order storage/connection diagnosis', 'host_product_install_performed': False}
headers = {'X-HLS-Test-Token': 'candidate-local-20261029-ui'}

def save(name, value):
    (report / name).write_text(json.dumps(value, indent=2), encoding='utf-8')

def run(arguments, name, timeout=180):
    with (report / (name+'.stdout.txt')).open('wb') as out, (report / (name+'.stderr.txt')).open('wb') as err:
        process = subprocess.run(arguments, stdout=out, stderr=err, timeout=timeout)
    if process.returncode:
        raise RuntimeError(f'{name} exit {process.returncode}')

def stop_runtime():
    subprocess.run(['powershell.exe','-NoProfile','-Command',r"Get-CimInstance Win32_Process | Where-Object {$_.ExecutablePath -and $_.ExecutablePath.StartsWith('C:\QACandidateInstalled\',[StringComparison]::OrdinalIgnoreCase)} | ForEach-Object {Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue}"], check=True, timeout=30)

def read_state():
    request = urllib.request.Request('http://127.0.0.1:18744/state', headers=headers)
    with urllib.request.urlopen(request, timeout=2) as response:
        return json.load(response)

msi = Path(r'C:\QACandidate\HLSDownloader-7.0.3-Windows-x64-candidate.msi')
installed = False
try:
    result['msi_sha256'] = hashlib.sha256(msi.read_bytes()).hexdigest()
    assert result['msi_sha256'] == '15410198df9d67d3998a0a19087807e6ca375689399c4e9a534f8ae26cbd161b'
    local_msi = work / 'candidate.msi'
    local_msi.write_bytes(msi.read_bytes())
    run(['msiexec.exe','/i',str(local_msi),'/qn','INSTALLDIR='+str(install),'/norestart','/l*v',str(report/'install.log')], 'install')
    installed = True
    engine = str(install / 'app/resources/HLSDownloaderEngine.exe')
    for mode in ('create-pause','verify-resume'):
        run([sys.executable,r'C:\QAScripts\msi_checkpoint_fixture.py','--mode',mode,'--engine',engine,'--root',str(work/'checkpoint'),'--state',str(work/'checkpoint/state.json'),'--core-port','18671','--origin-port','18672','--report',str(report/('checkpoint-'+mode+'.json'))], 'checkpoint-'+mode)
    os.environ.update(HLS_V7_DATA_DIR=str(work/'checkpoint/data'), HLS_V7_DOWNLOAD_DIR=str(work/'checkpoint/downloads'), HLS_V7_CORE_TCP='1', HLS_V7_CORE_BIND='127.0.0.1:18679', HLS_UI_TEST_API='1', HLS_UI_TEST_TOKEN=headers['X-HLS-Test-Token'], HLS_UI_TEST_PORT='18744', SLINT_BACKEND='winit-software')
    core = subprocess.Popen([engine], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    socket = connect(18679)
    send(socket, {'type':'store_settings','request_id':1,'values':{'legal_terms_accepted':True,'download_progress_window_enabled':False,'download_complete_popup_enabled':False}})
    for index in range(1,4):
        reply = send(socket, {'type':'command','request_id':index+1,'command':{'kind':'create_task','spec':{'url':f'https://fixture.invalid/selection-{index}.bin','filename':f'selection-{index}.bin','title':f'Sandbox selection fixture {index}','resource_kind':'file','scheduled_start_at':'2050-01-01T00:00:00Z'}}})
        assert reply['type'] != 'error', reply
    gui = subprocess.Popen([str(install/'HLSDownloader.exe')], cwd=install)
    result['phase'] = 'native-input-ready'
    save('candidate.json',result)
    restart_done = False
    deadline = time.monotonic()+1800
    while not (report/'native-finished.json').is_file() and time.monotonic()<deadline:
        try:
            save('state-latest.json',read_state())
            settings = send(socket,{'type':'load_settings','request_id':20})
            save('settings-latest.json',settings)
            save('core-latest.json',send(socket,{'type':'snapshot','request_id':21}))
            if (report/'restart-ui.json').is_file() and not restart_done:
                save('settings-before-restart.json',settings)
                gui.terminate();gui.wait(timeout=15)
                gui = subprocess.Popen([str(install/'HLSDownloader.exe')],cwd=install)
                restart_done = True
                save('restart-completed.json',{'restarted':True})
        except OSError:
            pass
        time.sleep(2)
    assert (report/'native-finished.json').is_file(), 'Native input deadline'
    result['native_input'] = json.loads((report/'native-finished.json').read_text(encoding='utf-8-sig'))
    save('settings-final.json',send(socket,{'type':'load_settings','request_id':30}))
    socket.close()
    stop_runtime()
    # One fixed order, no retries: durable disk baseline followed by paired 1/12 connection samples.
    performance = []
    for index, connections in enumerate(()):
        name = f'transfer-{index}-{connections}'
        arguments = [sys.executable,r'C:\QACandidateRunners\candidate-transfer-probe.py','--engine',engine,'--report',str(report/(name+'.json')),'--stage-root',str(work/'performance')]
        (work/'performance').mkdir(exist_ok=True)
        arguments += ['--storage-only'] if connections == 0 else ['--connections',str(connections),'--profile-io']
        try:
            run(arguments,name,90)
        except (RuntimeError,subprocess.TimeoutExpired) as error:
            performance.append({'sample':name,'error':str(error)})
        if (report/(name+'.json')).is_file():
            performance.append(json.loads((report/(name+'.json')).read_text(encoding='utf-8')))
    result['performance'] = performance
    output = work/'checkpoint/downloads/fixture.bin'
    before = hashlib.sha256(output.read_bytes()).hexdigest()
    run(['msiexec.exe','/x',str(local_msi),'/qn','/norestart','/l*v',str(report/'uninstall.log')],'uninstall')
    installed = False
    result['uninstall_preserves_output'] = before == hashlib.sha256(output.read_bytes()).hexdigest()
    assert result['uninstall_preserves_output'] and not (install/'HLSDownloader.exe').exists()
    result['passed'] = bool(result['native_input']['passed'])
except Exception as error:
    result['error'] = repr(error)
finally:
    if installed:
        try:
            stop_runtime()
            run(['msiexec.exe','/x',str(local_msi),'/qn','/norestart','/l*v',str(report/'cleanup-uninstall.log')],'cleanup-uninstall')
        except Exception as error:
            result['cleanup_error'] = repr(error)
    result['finished'] = True
    save('candidate.json',result)
