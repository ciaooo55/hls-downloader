"""Sandbox-only range tail comparison with an identical-binary control."""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import random
import socket
import statistics
import threading
import time
import traceback
import uuid

from smoke_v7_resource_boundaries import Core, wait


SIZE = 64 * 1024 * 1024
CHUNK = SIZE // 16
PAYLOAD = random.Random(20261025).randbytes(SIZE)
EXPECTED = hashlib.sha256(PAYLOAD).hexdigest()


def transfer(engine, root, mode, via_proxy=False, pause_on_help=False):
    requests = []
    helper_seen = threading.Event()
    release_helpers = threading.Event()
    if not pause_on_help:
        release_helpers.set()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_HEAD(self):
            self.send_response(200)
            self.send_header('Content-Length', str(SIZE))
            self.send_header('Accept-Ranges', 'bytes')
            self.send_header('ETag', '"range-tail-fixture"')
            self.end_headers()

        def do_GET(self):
            value = self.headers.get('Range', '')
            start, end = 0, SIZE - 1
            if value:
                first, last = value.removeprefix('bytes=').split('-')
                start, end = int(first), int(last) if last else end
            slow = mode == 'slow-last-connection' and start == SIZE - CHUNK
            request = {'start': start, 'end': end, 'sent': 0, 'slow_connection': slow}
            requests.append(request)
            self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            self.connection.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 65536)
            self.send_response(206 if value else 200)
            self.send_header('Content-Length', str(end - start + 1))
            self.send_header('Accept-Ranges', 'bytes')
            self.send_header('ETag', '"range-tail-fixture"')
            if value:
                self.send_header('Content-Range', f'bytes {start}-{end}/{SIZE}')
            self.send_header('Connection', 'close')
            self.end_headers()
            if start % CHUNK and end - start > 65536:
                helper_seen.set()
                if not release_helpers.wait(15):
                    return
            rate = (1 if slow else 16) * 1024 * 1024
            began = time.perf_counter()
            try:
                for offset in range(start, end + 1, 65536):
                    body = PAYLOAD[offset:min(offset + 65536, end + 1)]
                    self.wfile.write(body)
                    request['sent'] += len(body)
                    remaining = request['sent'] / rate - (time.perf_counter() - began)
                    if remaining > 0 and end - start > 65536:
                        time.sleep(remaining)
            except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                request['client_closed_early'] = True

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with Core(engine, root) as core:
            origin = f'http://127.0.0.1:{server.server_port}'
            cpu_before = sum(core.ps.cpu_times()[:2])
            began = time.perf_counter()
            created = core.command({'kind': 'create_task', 'spec': {
                'url': origin + '/payload.bin', 'filename': 'payload.bin', 'resource_kind': 'file',
                'concurrency': 4, 'proxy': origin if via_proxy else 'hls-downloader://direct-proxy'}})
            tid = next(e['event']['snapshot']['task_id'] for e in created['events']
                       if e['event']['kind'] == 'task_created')

            resumed = None
            if pause_on_help:
                wait(helper_seen.is_set, 20)
                core.command({'kind': 'task_action', 'task_id': tid, 'action': 'pause'})

                def paused():
                    task = next(t for t in core.snapshot()['tasks'] if t['task_id'] == tid)
                    return task if task['status'] == 'paused' and task['active_workers'] == 0 else None

                paused_task = wait(paused, 10)
                cache = root/'downloads/.hls-tasks'/tid
                checkpoints = list(cache.rglob('native-engine.ranges.json'))
                assert len(checkpoints) == 1 and json.loads(checkpoints[0].read_text())['ranges']
                assert paused_task['downloaded_bytes'] > SIZE // 2, paused_task
                release_helpers.set()
                resume_index = len(requests)
                core.command({'kind': 'task_action', 'task_id': tid, 'action': 'resume'})
                resumed = {'paused_downloaded_bytes': paused_task['downloaded_bytes'],
                           'durable_checkpoint_preserved': True, 'resume_request_index': resume_index}

            def finished():
                task = next(t for t in core.snapshot()['tasks'] if t['task_id'] == tid)
                return task if task['status'] in ('completed', 'failed') else None

            task = wait(finished, 45)
            elapsed = time.perf_counter() - began
            cpu = sum(core.ps.cpu_times()[:2]) - cpu_before
            assert task['status'] == 'completed', task
            output = Path(task['output_path'])
            with output.open('rb') as stream:
                actual = hashlib.file_digest(stream, 'sha256').hexdigest()
            assert output.stat().st_size == SIZE and actual == EXPECTED
            wait(lambda: not (root/'downloads/.hls-tasks'/tid).exists())
            core.command({'kind': 'task_action', 'task_id': tid, 'action': 'delete_files'})
            assert not output.exists()
            if resumed is not None:
                resumed['server_bytes_after_resume'] = sum(r['sent'] for r in requests[resume_index:])
                assert resumed['server_bytes_after_resume'] < SIZE // 2, resumed
            return {'elapsed_seconds': elapsed, 'cpu_seconds': cpu, 'rss_bytes': core.ps.memory_info().rss,
                    'requests': requests, 'server_payload_bytes_written': sum(r['sent'] for r in requests),
                    'bytes': SIZE, 'sha256': actual, 'task_and_cache_removed': True, 'resume_validation': resumed}
    finally:
        release_helpers.set()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    engines = {'baseline-a': args.baseline, 'baseline-b': args.baseline, 'candidate': args.candidate}
    root = Path(r'C:\QAWork')/('range-tail-' + uuid.uuid4().hex)
    result = {'passed': False, 'samples': [], 'medians': {},
              'scope': 'Core-only 64MiB loopback HTTP ranges with 4 connections; not TLS/Internet or IDM-AB parity',
              'engine_sha256': {key: hashlib.sha256(value.read_bytes()).hexdigest()
                                for key, value in engines.items()}}

    def save():
        args.report.write_text(json.dumps(result, indent=2), encoding='utf-8')

    try:
        for round_index, order in enumerate((('baseline-a', 'baseline-b', 'candidate'),
                                            ('baseline-b', 'candidate', 'baseline-a'),
                                            ('candidate', 'baseline-a', 'baseline-b')), 1):
            for mode, proxy in (('uniform', False), ('slow-last-connection', False),
                                ('slow-last-connection', True)):
                transport = 'winhttp-proxy' if proxy else 'raw-http'
                for label in order:
                    sample = {'round': round_index, 'label': label, 'mode': mode, 'transport': transport}
                    sample.update(transfer(engines[label], root/f'{round_index}-{mode}-{transport}-{label}', mode, proxy))
                    result['samples'].append(sample)
                    save()
                    print(json.dumps({key: value for key, value in sample.items() if key != 'requests'}), flush=True)
        for mode, transport in { (s['mode'], s['transport']) for s in result['samples'] }:
            result['medians'][f'{mode}/{transport}'] = {
                label: {field: statistics.median(s[field] for s in result['samples'] if
                        s['mode'] == mode and s['transport'] == transport and s['label'] == label)
                        for field in ('elapsed_seconds', 'cpu_seconds', 'rss_bytes', 'server_payload_bytes_written')}
                for label in engines}
        result['pause_resume_during_tail_help'] = [
            {'transport': 'winhttp-proxy' if proxy else 'raw-http',
             **transfer(args.candidate, root/f'pause-resume-{proxy}', 'slow-last-connection', proxy, True)}
            for proxy in (False, True)]
        result['passed'] = True
        save()
    except Exception:
        result['error'] = traceback.format_exc()
        save()
        raise


if __name__ == '__main__':
    main()
