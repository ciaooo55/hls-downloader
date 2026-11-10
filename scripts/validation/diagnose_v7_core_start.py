"""Diagnose packaged child-process startup on an isolated Windows runner."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import uuid

from native_message_io import frame
from smoke_v7_native_host import stop_isolated_engine
from validation_workspace import test_tmp
from v7_core_ipc import free_port

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--resources', type=Path, required=True)
parser.add_argument('--report', type=Path, required=True)
args = parser.parse_args()
resources = args.resources.resolve()
report = {'scope': 'Startup diagnosis only; no GUI/browser acceptance', 'attempts': []}
in_job = ctypes.c_int()
kernel = ctypes.WinDLL('kernel32', use_last_error=True)
kernel.IsProcessInJob(ctypes.c_void_p(-1), None, ctypes.byref(in_job))
report['parent_in_job'] = bool(in_job.value)
with tempfile.TemporaryDirectory(prefix='hls-start-diagnostic-', dir=test_tmp()) as temporary:
    root = Path(temporary)
    for name in ('HLSDownloaderEngine.exe', 'HLSDownloaderCli.exe', 'HLSDownloaderNativeHost.exe', 'vcruntime140.dll'):
        shutil.copy2(resources / name, root / name)
    engine = root / 'HLSDownloaderEngine.exe'
    env = os.environ.copy()
    env.update(HLS_V7_DATA_DIR=str(root / 'data'), HLS_V7_DOWNLOAD_DIR=str(root / 'downloads'),
               HLS_V7_CORE_TCP='1', HLS_V7_CORE_BIND=f'127.0.0.1:{free_port()}',
               HLS_V7_PIPE=rf'\\.\pipe\HLSDiagnosis-{uuid.uuid4().hex}',
               HLS_V7_STARTUP_PROFILE='1', HLS_VM_ACCEPTANCE='1')
    try:
        for flags in (0, 0x08000000, 0x09000000):
            attempt = {'creation_flags': hex(flags), 'command': 'Engine --self-test'}
            try:
                p = subprocess.run([str(engine), '--self-test'], creationflags=flags, env=env,
                                   capture_output=True, timeout=15, cwd=root)
                attempt.update(exit_code=p.returncode, stdout=p.stdout.decode('utf-8', 'replace'),
                               stderr=p.stderr.decode('utf-8', 'replace'))
            except OSError as error:
                attempt.update(error=str(error), winerror=error.winerror)
            report['attempts'].append(attempt)
        cli = subprocess.run([str(root / 'HLSDownloaderCli.exe'), '--json', 'list'],
                             env=env, capture_output=True, timeout=25, cwd=root)
        report['cli'] = {'exit_code': cli.returncode, 'stdout': cli.stdout.decode('utf-8', 'replace'),
                         'stderr': cli.stderr.decode('utf-8', 'replace')}
        stop_isolated_engine(engine)
        env.pop('HLS_V7_CORE_TCP', None)
        env.pop('HLS_V7_CORE_BIND', None)
        host = subprocess.run([str(root / 'HLSDownloaderNativeHost.exe')], env=env,
                              input=frame({'op': 'ping'}), capture_output=True, timeout=30, cwd=root)
        body = host.stdout
        length = struct.unpack('<I', body[:4])[0] if len(body) >= 4 else 0
        report['host'] = {'exit_code': host.returncode,
                          'response': body[4:4 + length].decode('utf-8', 'replace'),
                          'stderr': host.stderr.decode('utf-8', 'replace')}
    finally:
        stop_isolated_engine(engine)
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
print(json.dumps(report, ensure_ascii=True))
