"""Diagnose packaged child-process startup on an isolated Windows runner."""
import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import uuid

from native_message_io import frame, read_exact
from smoke_v7_native_host import stop_isolated_engine
from validation_workspace import test_tmp
from v7_core_ipc import free_port
from windows_breakaway_job import ExtendedLimits, job_api

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--resources', type=Path, required=True)
parser.add_argument('--report', type=Path, required=True)
args = parser.parse_args()
resources = args.resources.resolve()
report = {'scope': 'Startup diagnosis only; no GUI/browser acceptance', 'attempts': []}
in_job = wintypes.BOOL()
kernel = job_api()
if not kernel.IsProcessInJob(kernel.GetCurrentProcess(), None, ctypes.byref(in_job)):
    raise ctypes.WinError(ctypes.get_last_error())
report['parent_in_job'] = bool(in_job.value)
if in_job.value:
    limits = ExtendedLimits()
    if not kernel.QueryInformationJobObject(None, 9, ctypes.byref(limits), ctypes.sizeof(limits), None):
        raise ctypes.WinError(ctypes.get_last_error())
    report['parent_job_limit_flags'] = hex(limits.BasicLimitInformation.LimitFlags)
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
        # A profiled resident Core inherits stderr. A pipe would remain open
        # after the short-lived CLI/Host exits and make communicate() time out.
        cli_log = root / 'cli-stderr.log'
        cli_output = root / 'cli-stdout.log'
        with cli_log.open('wb') as diagnostics, cli_output.open('wb') as output:
            cli = subprocess.run([str(root / 'HLSDownloaderCli.exe'), '--json', 'list'],
                                 env=env, stdout=output, stderr=diagnostics, timeout=25, cwd=root)
        report['cli'] = {'exit_code': cli.returncode, 'stdout': cli_output.read_text(encoding='utf-8', errors='replace'),
                         'stderr': cli_log.read_text(encoding='utf-8', errors='replace')}
        stop_isolated_engine(engine)
        env.pop('HLS_V7_CORE_TCP', None)
        env.pop('HLS_V7_CORE_BIND', None)
        host_log = root / 'host-stderr.log'
        with host_log.open('wb') as diagnostics:
            host = subprocess.Popen([str(root / 'HLSDownloaderNativeHost.exe')], env=env,
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics, cwd=root)
            try:
                host.stdin.write(frame({'op': 'ping'}))
                host.stdin.close()
                header = read_exact(host.stdout, 4, 20)
                length = struct.unpack('<I', header)[0] if len(header) == 4 else 0
                if length > 1024 * 1024:
                    raise RuntimeError('Host response exceeds the native messaging limit')
                body = read_exact(host.stdout, length, 10) if length else b''
                host.wait(timeout=5)
                try:
                    eof = read_exact(host.stdout, 1, 2) == b''
                except TimeoutError:
                    eof = False
                report['host'] = {'exit_code': host.returncode, 'response': body.decode('utf-8', 'replace'),
                                  'protocol_eof_after_host_exit': eof,
                                  'stderr': host_log.read_text(encoding='utf-8', errors='replace')}
            finally:
                if host.poll() is None:
                    host.kill()
                host.wait(timeout=5)
                stop_isolated_engine(engine)
                host.stdout.close()
    finally:
        stop_isolated_engine(engine)
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
print(json.dumps(report, ensure_ascii=True))
