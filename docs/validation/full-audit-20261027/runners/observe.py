"""Read-only UI observation and all-runtime-process resource sample."""
import json
import os
from pathlib import Path
import time
import sys
from urllib.request import Request, urlopen

import psutil
from smoke_v7_portable_app import Pipe

assert os.environ.get('HLS_VM_ACCEPTANCE') == '1'
reports = Path(r'C:\QAReports')
session = json.loads((reports/'ui-session-ready.json').read_text('utf8'))
result = {}
label = sys.argv[1] if len(sys.argv)>1 else str(time.time_ns())
pipe = Pipe()
try:
    snapshot = pipe.request({'type':'snapshot','request_id':1})
    result['tasks'] = [{k:t.get(k) for k in ('task_id','filename','status','stage','total_bytes','downloaded_bytes')}
                       for t in snapshot['tasks']]
finally:
    pipe.close()
for endpoint in ('state', 'window'):
    request = Request(f'http://127.0.0.1:{session["port"]}/{endpoint}',
                      headers={'X-HLS-Test-Token':session['token']})
    with urlopen(request, timeout=10) as response:
        result[endpoint] = json.load(response)
request = Request(f'http://127.0.0.1:{session["port"]}/screenshot',
                  headers={'X-HLS-Test-Token':session['token']})
with urlopen(request, timeout=10) as response:
    (reports/f'ui-api-capture-{label}.png').write_bytes(response.read())


def sample():
    processes = {}
    for proc in psutil.process_iter(['exe']):
        try:
            if not proc.info['exe'] or not Path(proc.info['exe']).is_relative_to(r'C:\QAWork\runtime'):
                continue
            cpu = proc.cpu_times()
            memory = proc.memory_info()
            processes[proc.pid] = {'name':proc.name(), 'cpu_seconds':cpu.user+cpu.system,
                'private_bytes':memory.private, 'rss_bytes':memory.rss,
                'handles':proc.num_handles(), 'threads':proc.num_threads()}
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            pass
    return processes


first = sample()
started = time.monotonic()
time.sleep(5)
last = sample()
elapsed = time.monotonic()-started
cpu = sum(max(0, p['cpu_seconds']-first.get(pid,p)['cpu_seconds']) for pid,p in last.items())
result['product_process_sample'] = {'seconds':elapsed, 'cpu_one_core_percent':100*cpu/elapsed,
    'cpu_machine_percent':100*cpu/elapsed/psutil.cpu_count(), 'processes':last,
    'private_bytes':sum(p['private_bytes'] for p in last.values()),
    'rss_sum_bytes':sum(p['rss_bytes'] for p in last.values()),
    'handles':sum(p['handles'] for p in last.values()), 'threads':sum(p['threads'] for p in last.values())}
(reports/f'ui-observation-{label}.json').write_text(json.dumps(result,ensure_ascii=False,indent=2), 'utf8')
