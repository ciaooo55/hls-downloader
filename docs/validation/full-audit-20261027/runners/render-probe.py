"""Sandbox-only renderer A/B; keep task data, geometry and runtime identical."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from urllib.request import Request, urlopen

import psutil

assert os.environ.get('HLS_VM_ACCEPTANCE') == '1'
root = Path(r'C:\QAReports')
runtime = Path(r'C:\QAWork\runtime')
session = json.loads((root/'ui-session-ready.json').read_text('utf8'))
config = runtime/'app/HLSDownloader.cfg'
original = config.read_text('utf8')
results = []
label = sys.argv[1] if len(sys.argv)>1 else 'initial'


def stop_ui():
    for proc in psutil.process_iter(['exe']):
        if proc.info['exe'] and Path(proc.info['exe']) == runtime/'HLSDownloader.exe':
            try:
                proc.kill()
                proc.wait(10)
            except psutil.NoSuchProcess:
                pass


def read_state():
    req = Request(f'http://127.0.0.1:{session["port"]}/state',
                  headers={'X-HLS-Test-Token':session['token']})
    with urlopen(req, timeout=3) as response:
        return json.load(response)


try:
    for renderer in ('SOFTWARE', 'SOFTWARE_COMPAT', 'DIRECT3D'):
        stop_ui()
        configured = re.sub(r'-Dskiko.renderApi=\w+', '-Dskiko.renderApi='+renderer, original)
        configured += '\njava-options=-Dskiko.fps.enabled=true\n'
        config.write_text(configured, 'utf8')
        env = {**os.environ, 'HLS_UI_TEST_API':'1', 'HLS_UI_TEST_PORT':str(session['port']),
               'HLS_UI_TEST_TOKEN':session['token']}
        with (root/f'render-{label}-{renderer}.log').open('wb') as log:
            subprocess.Popen([str(runtime/'HLSDownloader.exe')],cwd=runtime,env=env,stdout=log,stderr=log)
        deadline = time.monotonic()+90
        while True:
            try:
                state = read_state()
                if state.get('controlBounds') and '已连接' in state.get('engineText',''):
                    break
            except OSError:
                pass
            if time.monotonic()>deadline:
                raise RuntimeError(f'{renderer}: UI did not render/connect')
            time.sleep(.25)
        time.sleep(8)
        samples = []
        for _ in range(3):
            procs = [p for p in psutil.process_iter(['exe']) if p.info['exe']
                     and Path(p.info['exe']).is_relative_to(runtime)]
            start_cpu = {p.pid:sum(p.cpu_times()[:2]) for p in procs}
            start = time.monotonic()
            time.sleep(5)
            elapsed = time.monotonic()-start
            cpu = sum(max(0,sum(p.cpu_times()[:2])-start_cpu[p.pid]) for p in procs if p.is_running())
            samples.append({'seconds':elapsed, 'cpu_one_core_percent':cpu/elapsed*100,
                'cpu_machine_percent':cpu/elapsed*100/psutil.cpu_count(),
                'private_bytes':sum(p.memory_info().private for p in procs if p.is_running())})
        req = Request(f'http://127.0.0.1:{session["port"]}/screenshot',
                      headers={'X-HLS-Test-Token':session['token']})
        with urlopen(req, timeout=10) as response:
            (root/f'render-{label}-{renderer}.png').write_bytes(response.read())
        results.append({'renderer':renderer,'state':state,'samples':samples})
        (root/f'render-comparison-{label}.json').write_text(json.dumps(results,ensure_ascii=False,indent=2),'utf8')
finally:
    config.write_text(original,'utf8')
