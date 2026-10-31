"""Sandbox-only portable shutdown wiring check with the real candidate Core."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading
import traceback

from v7_core_ipc import connect, send_frame as send, wait_task
from msi_checkpoint_fixture import BLOCK, PAYLOAD_SIZE, RangeHandler, Server


def main():
    assert os.environ.get('USERNAME') == 'WDAGUtilityAccount', 'Windows Sandbox only'
    parser = argparse.ArgumentParser()
    parser.add_argument('--image', type=Path, required=True)
    parser.add_argument('--scripts', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=False)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    source, target = args.work/'source image', args.work/'target image'
    result = dict(passed=False, finished=False, checks=[], scope='Current portable scripts/native preflight with actual candidate Core in minimal app images; not a packaged-image acceptance')
    core = None
    lock = None
    server = None
    gates = []
    env = dict(os.environ, HLS_V7_PIPE=r'\\.\pipe\HLSPortableUpgradeAcceptance',
               HLS_V7_DATA_DIR=str(target/'data'), HLS_V7_DOWNLOAD_DIR=str(target/'downloads'),
               HLS_V7_CORE_TCP='1', HLS_V7_CORE_BIND='127.0.0.1:18681',)
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.CreateFileW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
    kernel.CreateFileW.restype = ctypes.c_void_p
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]

    def save():
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')

    def run(script, arguments, name, expected):
        result['phase'] = name
        save()
        wrapper = args.work/(name+'.ps1')
        quoted = lambda value: "'"+str(value).replace("'", "''")+"'"
        wrapper.write_text("[Threading.Thread]::CurrentThread.CurrentUICulture=[Globalization.CultureInfo]::GetCultureInfo('en-US')\n& "+quoted(script)+' '+arguments+'\n', encoding='utf-8')
        with (args.work/(name+'.stdout.txt')).open('wb') as out, (args.work/(name+'.stderr.txt')).open('wb') as err:
            proc = subprocess.Popen(['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(wrapper)], env=env, stdout=out, stderr=err, creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                code = proc.wait(timeout=180)
            except subprocess.TimeoutExpired:
                proc.terminate()
                proc.wait(timeout=10)
                raise
        result['checks'].append(dict(name=name, exit_code=code))
        assert (code == 0) == expected, (name, code)

    def start():
        return subprocess.Popen([str(target/'app/resources/HLSDownloaderEngine.exe')], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW)

    try:
        result['components'] = {name:hashlib.sha256((args.scripts/name).read_bytes()).hexdigest()
                                for name in ('upgrade-v7-portable.ps1', 'HLSDownloaderPreflight.exe')}
        result['core_sha256'] = hashlib.sha256((args.image/'app/resources/HLSDownloaderEngine.exe').read_bytes()).hexdigest()
        # The upgrade validates real provenance and extension identities, but
        # copying an unused JRE adds no coverage for the shutdown boundary.
        for root in (source, target):
            (root/'app/resources').mkdir(parents=True)
            (root/'runtime').mkdir()
            (root/'scripts').mkdir()
            (root/'extensions').mkdir()
            shutil.copyfile(args.image/'HLSDownloader.exe', root/'HLSDownloader.exe')
            for name in ('HLSDownloaderEngine.exe', 'vcruntime140.dll', 'BUILD-PROVENANCE.json', 'FEATURE-PARITY.json'):
                shutil.copyfile(args.image/'app/resources'/name, root/'app/resources'/name)
            for archive in (args.image/'app/resources/extensions').glob('*.zip'):
                shutil.copyfile(archive, root/'extensions'/archive.name)
            for name in ('upgrade-v7-portable.ps1', 'V7HashFunctions.ps1', 'HLSDownloaderPreflight.exe'):
                shutil.copyfile(args.scripts/name, root/'scripts'/name)
        (target/'old-image.marker').write_bytes(b'old image')
        class HeldRange(RangeHandler):
            def do_GET(self):
                gate = self.server.release
                value = self.headers.get('Range', '')
                first, _, last = value.removeprefix('bytes=').partition('-')
                start = int(first or 0) if value.startswith('bytes=') else 0
                end = min(int(last) if last else PAYLOAD_SIZE-1, PAYLOAD_SIZE-1)
                self._headers(206 if value.startswith('bytes=') else 200, start, end)
                sent = 0
                try:
                    for offset in range(start, end+1, len(BLOCK)):
                        count = min(len(BLOCK), end-offset+1)
                        pos = offset % len(BLOCK)
                        self.wfile.write((BLOCK[pos:]+BLOCK[:pos])[:count])
                        self.wfile.flush()
                        sent += count
                        # Keep a real response incomplete until shutdown saves
                        # its partial range, independent of PowerShell startup.
                        if sent >= 1024*1024 and not gate.wait(180):
                            return
                except (BrokenPipeError, ConnectionResetError):
                    pass
        server = Server(('127.0.0.1', 18682), HeldRange)
        server.release = threading.Event()
        gates.append(server.release)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        core = start()
        ipc = connect(18681)
        created = send(ipc, {'type':'command', 'request_id':1, 'command':{'kind':'create_task', 'spec':{
            'url':'http://127.0.0.1:18682/fixture.bin', 'filename':'fixture.bin', 'resource_kind':'file', 'concurrency':1}}})
        task_id = next(e['event']['snapshot']['task_id'] for e in created['events'] if e['event']['kind']=='task_created')
        wait_task(ipc, task_id, lambda task: task['downloaded_bytes'] >= 256*1024, 10)
        ipc.close()
        lock = kernel.CreateFileW(str(target/'HLSDownloader.exe'), 0x80000000, 0, None, 3, 0, None)
        assert lock not in (None, ctypes.c_void_p(-1).value), ctypes.get_last_error()
        arguments = '-SourceDir "'+str(source)+'" -TargetDir "'+str(target)+'"'
        run(source/'scripts/upgrade-v7-portable.ps1', arguments, 'lock-refuses-swap', False)
        assert source.is_dir() and (target/'old-image.marker').read_bytes()==b'old image'
        assert not Path(str(target)+'.v7-backup').exists()
        core.wait(timeout=10)
        kernel.CloseHandle(lock)
        lock = None
        server.release.set()
        server.release = threading.Event()
        gates.append(server.release)
        core = start()
        ipc = connect(18681)
        restored = wait_task(ipc, task_id, lambda task: task['status'] in ('paused','completed'), 15)
        assert 256*1024 <= restored['downloaded_bytes'] < PAYLOAD_SIZE
        result['after_failed_swap_checkpoint_bytes'] = restored['downloaded_bytes']
        send(ipc, {'type':'command', 'request_id':2, 'command':{'kind':'task_action', 'task_id':task_id, 'action':'resume'}})
        ipc.close()
        run(source/'scripts/upgrade-v7-portable.ps1', arguments, 'running-upgrade', True)
        core.wait(timeout=10)
        assert Path(str(target)+'.v7-backup').joinpath('old-image.marker').exists()
        server.release.set()
        core = start()
        ipc = connect(18681)
        restored = wait_task(ipc, task_id, lambda task: task['status'] in ('paused','completed'), 15)
        assert 256*1024 <= restored['downloaded_bytes'] < PAYLOAD_SIZE
        result['after_running_upgrade_checkpoint_bytes'] = restored['downloaded_bytes']
        send(ipc, {'type':'command', 'request_id':3, 'command':{'kind':'task_action', 'task_id':task_id, 'action':'resume'}})
        completed = wait_task(ipc, task_id, lambda task: task['status'] in ('completed','failed'), 100)
        ipc.close()
        assert completed['status']=='completed', completed
        output = target/'downloads/fixture.bin'
        expected = hashlib.sha256(BLOCK*(PAYLOAD_SIZE//len(BLOCK))).hexdigest()
        assert output.stat().st_size==PAYLOAD_SIZE and hashlib.sha256(output.read_bytes()).hexdigest()==expected
        run(target/'scripts/upgrade-v7-portable.ps1', '-Rollback -RollbackDir "'+str(target)+'"', 'running-rollback', True)
        core.wait(timeout=10)
        assert (target/'old-image.marker').exists() and hashlib.sha256(output.read_bytes()).hexdigest()==expected
        result.update(passed=True, task_id=task_id, output_sha256=expected,
                      helper_sha256=hashlib.sha256((target/'scripts/HLSDownloaderPreflight.exe').read_bytes()).hexdigest())
    except Exception as error:
        result.update(error=repr(error), traceback=traceback.format_exc())
    finally:
        for gate in gates:
            gate.set()
        if lock not in (None, ctypes.c_void_p(-1).value):
            kernel.CloseHandle(lock)
        if core and core.poll() is None:
            core.terminate()
            core.wait(timeout=10)
        if server:
            server.shutdown()
        result['finished'] = True
        save()
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
