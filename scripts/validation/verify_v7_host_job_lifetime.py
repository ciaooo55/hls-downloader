"""Verify that terminating the Native Host job leaves the same Core alive."""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import uuid

import psutil

from native_message_io import frame, read_exact
from smoke_v7_native_host import responses, stop_isolated_engine
from validation_workspace import test_tmp
from windows_breakaway_job import breakaway_job, job_api


def child(root, job_name):
    with breakaway_job(job_name, kill_on_close=True):
        with (root / 'host-stderr.log').open('wb') as diagnostics:
            host = subprocess.Popen([str(root / 'HLSDownloaderNativeHost.exe')],
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=diagnostics)
            host.stdin.write(frame({'op': 'ping'}))
            host.stdin.flush()
            header = read_exact(host.stdout, 4, 20)
            if len(header) != 4:
                raise RuntimeError('Job-contained Host did not return a frame')
            size = struct.unpack('<I', header)[0]
            if not 0 < size <= 1024 * 1024:
                raise RuntimeError('Job-contained Host returned an invalid frame size')
            reply = json.loads(read_exact(host.stdout, size, 10))
            if reply.get('ok') is not True:
                raise RuntimeError(f'Job-contained Host failed: {reply}')
            engine_path = root / 'HLSDownloaderEngine.exe'
            engines = []
            for process in psutil.process_iter(['pid', 'exe']):
                if process.info['exe'] and Path(process.info['exe']) == engine_path:
                    engines.append(process.info['pid'])
            if len(engines) != 1:
                raise RuntimeError(f'Expected one cold-started Core, found {engines}')
            ready = root / 'ready.json'
            pending = root / 'ready.pending'
            pending.write_text(json.dumps({'host_pid': host.pid, 'engine_pid': engines[0]}), encoding='utf-8')
            pending.replace(ready)
            # Keep both Host and its fixture alive until the parent kills the
            # whole job. The parent owns this pipe; no desktop input is read.
            sys.stdin.buffer.read(1)


def verify(resources, report_path):
    kernel = job_api()
    report = {'schema': 1, 'scope': 'Packaged Host/Core job-lifetime regression; no browser or GUI acceptance', 'passed': False}
    with tempfile.TemporaryDirectory(prefix='hls-host-job-', dir=test_tmp()) as temporary:
        root = Path(temporary).resolve()
        engine = root / 'HLSDownloaderEngine.exe'
        host = root / 'HLSDownloaderNativeHost.exe'
        for name in ('HLSDownloaderEngine.exe', 'HLSDownloaderNativeHost.exe', 'vcruntime140.dll'):
            shutil.copy2(resources / name, root / name)
        environment = os.environ.copy()
        environment.pop('HLS_V7_CORE_TCP', None)
        environment.pop('HLS_V7_CORE_BIND', None)
        environment.update(HLS_V7_DATA_DIR=str(root / 'data'), HLS_V7_DOWNLOAD_DIR=str(root / 'downloads'),
                           HLS_V7_PIPE=rf'\\.\pipe\HLSJobLifetime-{uuid.uuid4().hex}')
        job_name = f'Local\\HLSValidation-{uuid.uuid4().hex}'
        fixture = None
        job = None
        handles = []
        try:
            with (root / 'fixture-stderr.log').open('wb') as diagnostics:
                fixture = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--child', str(root), job_name],
                                           stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=diagnostics, env=environment)
                deadline = time.monotonic() + 30
                while not (root / 'ready.json').is_file():
                    if fixture.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError('Job-lifetime fixture did not become ready: ' +
                                           (root / 'fixture-stderr.log').read_text(encoding='utf-8', errors='replace') +
                                           ((root / 'host-stderr.log').read_text(encoding='utf-8', errors='replace') if (root / 'host-stderr.log').exists() else ''))
                    time.sleep(0.05)
                ready = json.loads((root / 'ready.json').read_text(encoding='utf-8'))
                job = kernel.OpenJobObjectW(0x0004 | 0x0008, False, job_name)  # QUERY | TERMINATE
                if not job:
                    raise ctypes.WinError(ctypes.get_last_error())
                membership = {}
                for role in ('host', 'engine'):
                    handle = kernel.OpenProcess(0x00100000 | 0x1000, False, ready[f'{role}_pid'])
                    if not handle:
                        raise ctypes.WinError(ctypes.get_last_error())
                    handles.append(handle)
                    inside = wintypes.BOOL()
                    if not kernel.IsProcessInJob(handle, job, ctypes.byref(inside)):
                        raise ctypes.WinError(ctypes.get_last_error())
                    membership[role] = bool(inside.value)
                if membership != {'host': True, 'engine': False}:
                    raise RuntimeError(f'Host/Core lifetime separation failed: {membership}')
                if not kernel.TerminateJobObject(job, 73):
                    raise ctypes.WinError(ctypes.get_last_error())
                fixture.wait(timeout=5)
                if kernel.WaitForSingleObject(handles[0], 5000) != 0:
                    raise RuntimeError('Native Host survived termination of its job')
                if kernel.WaitForSingleObject(handles[1], 0) != 258:
                    raise RuntimeError('Core terminated with the Native Host job')
                replies, _, _, _ = responses(host, environment)
                if any(reply.get('ok') is not True for reply in replies):
                    raise RuntimeError('Could not reconnect to the surviving Core')
                if kernel.WaitForSingleObject(handles[1], 0) != 258:
                    raise RuntimeError('The original Core exited during reconnection')
                report.update(passed=True, host_in_terminated_job=True, core_in_terminated_job=False,
                              host_terminated=True, same_core_survived=True, reconnect_pings=len(replies),
                              host_sha256=hashlib.sha256(host.read_bytes()).hexdigest(),
                              engine_sha256=hashlib.sha256(engine.read_bytes()).hexdigest())
        except BaseException as error:
            report['error'] = str(error)
            raise
        finally:
            if job:
                kernel.TerminateJobObject(job, 74)
            if fixture is not None:
                if fixture.poll() is None:
                    fixture.kill()
                fixture.wait(timeout=5)
                fixture.stdin.close()
            for handle in handles:
                kernel.CloseHandle(handle)
            if job:
                kernel.CloseHandle(job)
            stop_isolated_engine(engine)
            report_path.parent.mkdir(parents=True, exist_ok=True)
            report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))


def main():
    if len(sys.argv) > 1 and sys.argv[1] == '--child':
        child(Path(sys.argv[2]), sys.argv[3])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--resources', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    verify(args.resources.resolve(), args.report)


if __name__ == '__main__':
    main()
