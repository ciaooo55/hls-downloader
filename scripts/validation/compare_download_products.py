from __future__ import annotations

import contextlib
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import threading
import time
import uuid
from urllib.request import Request, urlopen

import psutil
from smoke_v7_portable_app import Pipe

assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Windows Sandbox only'
parser = argparse.ArgumentParser()
for name in ('runtime', 'ab', 'idm', 'report'):
    parser.add_argument('--' + name, type=Path, required=True)
args = parser.parse_args()
ROOT = args.report
ROOT.mkdir(parents=True, exist_ok=True)
RUN_ID = uuid.uuid4().hex[:8]
OUT = Path(r'C:\QAWork\comparison-clean') / RUN_ID
PATTERN = bytes(range(256)) * 256
REPORT = {'schema': 1, 'scope': 'loopback default-settings transfer and whole-product process resources; not UI response or public Internet', 'samples': [], 'errors': []}
IDM = args.idm
APPS = {'hls': args.runtime/'HLSDownloader.exe', 'ab': args.ab, 'idm': IDM}
assert all(p.is_file() for p in APPS.values()), APPS

PREFIXES = {k: str(v.parent).lower() + '\\' for k, v in APPS.items()}
RUNS = {}
LOCK = threading.Lock()


def save():
    (ROOT / 'comparison-clean.json').write_text(json.dumps(REPORT, indent=2), encoding='utf-8')


def processes(product):
    result = []
    for p in psutil.process_iter(['exe']):
        with contextlib.suppress(psutil.Error):
            if (p.info['exe'] or '').lower().startswith(PREFIXES[product]):
                result.append(p)
    return result


def resources(product):
    value = {'rss': 0, 'private': 0, 'cpu': 0, 'threads': 0, 'handles': 0, 'read_bytes': 0, 'write_bytes': 0, 'pids': [], 'processes': []}
    for p in processes(product):
        with contextlib.suppress(psutil.Error):
            memory = p.memory_info()
            cpu = p.cpu_times()
            io = p.io_counters()
            value['rss'] += memory.rss
            value['private'] += memory.private
            value['cpu'] += cpu.user + cpu.system
            value['threads'] += p.num_threads()
            value['handles'] += p.num_handles()
            value['read_bytes'] += io.read_bytes
            value['write_bytes'] += io.write_bytes
            value['pids'].append(p.pid)
            value['processes'].append({'pid': p.pid, 'name': p.name(), 'cpu': cpu.user + cpu.system,
                                       'private': memory.private, 'rss': memory.rss})
    return value


def stop(product):
    owned = processes(product)
    for p in owned:
        with contextlib.suppress(psutil.Error):
            p.terminate()
    _, alive = psutil.wait_procs(owned, timeout=8)
    for p in alive:
        with contextlib.suppress(psutil.Error):
            p.kill()


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *_):
        pass

    def do_HEAD(self):
        self.serve(False)

    def do_GET(self):
        self.serve(True)

    def serve(self, body):
        key = self.path.rsplit('/', 1)[-1]
        with LOCK:
            state = RUNS.get(key)
        if state is None:
            self.send_error(404)
            return
        total = state['size']
        start, end, status = 0, total - 1, 200
        header = self.headers.get('Range', '')
        if header.startswith('bytes='):
            first, _, last = header[6:].partition('-')
            start, end, status = int(first or 0), min(int(last) if last else total - 1, total - 1), 206
        if start >= total or end < start:
            self.send_error(416)
            return
        if state['latency']:
            time.sleep(state['latency'])
        self.send_response(status)
        self.send_header('Content-Type', 'application/octet-stream')
        self.send_header('Content-Length', str(end - start + 1))
        self.send_header('Content-Disposition', f'attachment; filename="{key}"')
        self.send_header('Accept-Ranges', 'bytes')
        self.send_header('ETag', '"product-comparison-v1"')
        if status == 206:
            self.send_header('Content-Range', f'bytes {start}-{end}/{total}')
        self.end_headers()
        if not body:
            return
        request = {'start': start, 'end': end, 'status': status, 'sent': 0, 'started': time.perf_counter()}
        with LOCK:
            state['requests'].append(request)
        began = time.perf_counter()
        with contextlib.suppress(BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            for offset in range(start, end + 1, len(PATTERN)):
                size = min(len(PATTERN), end + 1 - offset)
                shift = offset % 256
                data = PATTERN[:size] if not shift else (PATTERN + PATTERN[:256])[shift:shift + size]
                self.wfile.write(data)
                self.wfile.flush()
                with LOCK:
                    request['sent'] += size
                    state.setdefault('first_byte', time.perf_counter())
                if state['rate']:
                    remaining = request['sent'] / state['rate'] - (time.perf_counter() - began)
                    if remaining > 0:
                        time.sleep(remaining)


def coverage(state):
    with LOCK:
        intervals = sorted((r['start'], r['start'] + r['sent']) for r in state['requests'] if r['sent'])
    covered, high = 0, 0
    for start, end in intervals:
        if end > high:
            covered += end - max(high, start)
            high = end
    return covered


def request_json(url, payload):
    with urlopen(Request(url, data=json.dumps(payload).encode(), headers={'Content-Type': 'application/json'}), timeout=10) as response:
        return response.read().decode()


def core(message):
    pipe = Pipe()
    try:
        return pipe.request(message)
    finally:
        pipe.close()


def start(product, round_index):
    log = (ROOT / f'{product}-{round_index}.log').open('wb')
    environment = {**os.environ, 'HLS_V7_DATA_DIR': str(OUT / 'hls-data'), 'HLS_V7_DOWNLOAD_DIR': str(OUT / 'hls'), 'HLS_V6_SKIP_MIGRATE': '1'}
    if product.startswith('hls'):
        subprocess.Popen([str(args.runtime/'app/resources/HLSDownloaderEngine.exe')], env=environment, stdout=log, stderr=log)
    subprocess.Popen([str(APPS[product])], env=environment, stdout=log, stderr=log)
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        try:
            if product == 'ab':
                request_json('http://127.0.0.1:15151/ping', {})
            elif product.startswith('hls'):
                assert core({'type': 'snapshot', 'request_id': 1}).get('type') == 'snapshot'
            else:
                assert processes(product)
            break
        except Exception:
            time.sleep(.2)
    else:
        raise RuntimeError(f'{product} did not become available')
    time.sleep(8)
    before = resources(product)
    time.sleep(5)
    after = resources(product)
    return {'sample_seconds': 5, 'before': before, 'after': after, 'cpu_seconds': after['cpu'] - before['cpu']}


def transfer(product, round_index, scenario, server):
    size = (128 if scenario == 'fast' else 64) * 2**20
    key = f'{product}-{round_index}-{scenario}-{RUN_ID}.bin'
    state = {'size': size, 'rate': 0 if scenario == 'fast' else 4 * 2**20, 'latency': 0 if scenario == 'fast' else .08, 'requests': []}
    RUNS[key] = state
    directory = OUT / product
    directory.mkdir(parents=True, exist_ok=True)
    output = directory / key
    url = f'http://127.0.0.1:{server.server_port}/{key}'
    before = resources(product)
    peak = dict(before)
    cpu_before = {p['pid']: p['cpu'] for p in before['processes']}
    cpu_observed = dict(cpu_before)
    began = time.perf_counter()
    if product.startswith('hls'):
        core({'type': 'store_setting', 'request_id': 1, 'key': 'download_dir', 'value': str(directory)})
        response = core({'type': 'command', 'request_id': 2, 'command': {'kind': 'create_task', 'spec': {'url': url, 'resource_kind': 'file', 'filename': key, 'expected_size': size}}})
        task_id = next(e['event']['snapshot']['task_id'] for e in response['events'] if e['event']['kind'] == 'task_created')
        core({'type': 'command', 'request_id': 3, 'command': {'kind': 'task_action', 'task_id': task_id, 'action': 'start'}})
    elif product == 'ab':
        request_json('http://127.0.0.1:15151/start-headless-download', {'downloadSource': {'type': 'http', 'link': url}, 'folder': str(directory), 'name': key, 'startDownload': True})
    else:
        subprocess.Popen([str(IDM), '/d', url, '/p', str(directory), '/f', key, '/n', '/h'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    submit_ms = (time.perf_counter() - began) * 1000
    sha = hashlib.sha256()
    for _ in range(size // len(PATTERN)):
        sha.update(PATTERN)
    expected = sha.hexdigest()
    deadline = time.monotonic() + 120
    t90 = None
    while time.monotonic() < deadline:
        current = resources(product)
        for process in current['processes']:
            cpu_observed[process['pid']] = max(cpu_observed.get(process['pid'], 0), process['cpu'])
        for metric in ['rss', 'private', 'threads', 'handles']:
            peak[metric] = max(peak[metric], current[metric])
        received = coverage(state)
        if received >= size * .9 and t90 is None:
            t90 = time.perf_counter()
        if received >= size and output.is_file() and output.stat().st_size == size:
            with contextlib.suppress(OSError):
                if hashlib.file_digest(output.open('rb'), 'sha256').hexdigest() == expected:
                    break
        time.sleep(.05)
    else:
        raise TimeoutError(f'{product} {scenario}: {coverage(state)}/{size}; files={list(directory.iterdir())}')
    finished = time.perf_counter()
    after = resources(product)
    for process in after['processes']:
        cpu_observed[process['pid']] = max(cpu_observed.get(process['pid'], 0), process['cpu'])
    sampled_cpu_seconds = sum(value - cpu_before.get(pid, 0) for pid, value in cpu_observed.items())
    time.sleep(.3)
    with LOCK:
        requests = [dict(r) for r in state['requests']]
    sample = {'product': product, 'round': round_index, 'scenario': scenario, 'payload_bytes': size, 'submission_ms': submit_ms, 'first_byte_ms': (state['first_byte'] - began) * 1000, 'verified_output_ms': (finished - began) * 1000, 'last_ten_percent_ms': (finished - t90) * 1000 if t90 else None, 'sha256': expected, 'before': before, 'peak': peak, 'after': after, 'cpu_seconds': sampled_cpu_seconds, 'cpu_live_process_delta': after['cpu'] - before['cpu'], 'requests': requests, 'server_bytes_sent': sum(r['sent'] for r in requests), 'unique_server_bytes': coverage(state)}
    REPORT['samples'].append(sample)
    save()
    print(json.dumps({k: sample[k] for k in ['product', 'round', 'scenario', 'verified_output_ms', 'cpu_seconds', 'server_bytes_sent']}), flush=True)
    if product.startswith('hls'):
        core({'type': 'command', 'request_id': 4, 'command': {'kind': 'task_action', 'task_id': task_id, 'action': 'delete_files'}})
    else:
        output.unlink()


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    for p in APPS:
        stop(p)
    class Server(ThreadingHTTPServer):
        def handle_error(self, *_):
            pass
    server = Server(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    REPORT['installed_bytes'] = {k: sum(p.stat().st_size for p in app.parent.rglob('*') if p.is_file()) for k, app in APPS.items()}
    REPORT['binaries'] = {k: {'path': str(v), 'sha256': hashlib.file_digest(v.open('rb'), 'sha256').hexdigest()} for k, v in APPS.items()}
    save()
    try:
        for round_index, order in enumerate([['hls', 'ab', 'idm'], ['ab', 'idm', 'hls'], ['idm', 'hls', 'ab']], 1):
            for product in order:
                try:
                    idle = start(product, round_index)
                    REPORT.setdefault('idle', []).append({'product': product, 'round': round_index, **idle})
                    save()
                    for scenario in ['fast', 'limited']:
                        transfer(product, round_index, scenario, server)
                except Exception as error:
                    REPORT['errors'].append({'product': product, 'round': round_index, 'error': repr(error)})
                    save()
                finally:
                    stop(product)
        REPORT['passed'] = len(REPORT['samples']) == 18 and not REPORT['errors']
        save()
    finally:
        server.shutdown()
        server.server_close()
        for p in APPS:
            stop(p)


if __name__ == '__main__':
    main()
