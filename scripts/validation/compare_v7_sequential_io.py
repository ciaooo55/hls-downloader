"""Sandbox-only sequential transfer comparison, including an identical-binary control."""
from __future__ import annotations

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import threading
import time
import traceback

from smoke_v7_resource_boundaries import Core, wait


SIZE = 128 * 1024 * 1024
PATTERN = bytes(range(256)) * 256


def transfer(engine, root, method, block_size):
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            self.serve()

        def do_POST(self):
            assert self.rfile.read(int(self.headers.get('Content-Length', '0'))) == b'fixture=ok'
            self.serve()

        def serve(self):
            request = {'method': self.command, 'range': self.headers.get('Range'), 'sent': 0}
            requests.append(request)
            self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            self.send_response(200)
            self.send_header('Content-Length', str(SIZE))
            self.send_header('Content-Type', 'application/octet-stream')
            self.send_header('ETag', '"sequential-io-fixture"')
            self.send_header('Connection', 'close')
            self.end_headers()
            try:
                for _ in range(SIZE // block_size):
                    self.wfile.write(PATTERN[:block_size])
                    request['sent'] += block_size
            except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with Core(engine, root) as core:
            body = root/'request-body'
            body.write_bytes(b'fixture=ok')
            cpu_before = sum(core.ps.cpu_times()[:2])
            io_before = core.ps.io_counters()
            began = time.perf_counter()
            created = core.command({'kind': 'create_task', 'spec': {
                'url': f'http://127.0.0.1:{server.server_port}/payload.bin',
                'filename': 'payload.bin', 'resource_kind': 'file',
                'request_method': method, 'body_path': str(body) if method == 'POST' else '',
                'proxy': 'hls-downloader://direct-proxy'}})
            tid = next(e['event']['snapshot']['task_id'] for e in created['events']
                       if e['event']['kind'] == 'task_created')

            def finished():
                task = next(t for t in core.snapshot()['tasks'] if t['task_id'] == tid)
                return task if task['status'] in ('completed', 'failed') else None

            task = wait(finished, 90)
            elapsed = time.perf_counter()-began
            cpu = sum(core.ps.cpu_times()[:2])-cpu_before
            io_after = core.ps.io_counters()
            assert task['status'] == 'completed', task
            output = Path(task['output_path'])
            assert output.stat().st_size == SIZE
            expected = hashlib.sha256()
            for _ in range(SIZE // len(PATTERN)):
                expected.update(PATTERN)
            with output.open('rb') as stream:
                actual = hashlib.file_digest(stream, 'sha256').hexdigest()
            assert actual == expected.hexdigest()
            full = [r for r in requests if r['range'] is None]
            assert len(full) == 1 and full[0]['sent'] == SIZE, requests
            assert len(requests) == (1 if method == 'POST' else 2), requests
            wait(lambda: not (root/'downloads/.hls-tasks'/tid).exists())
            core.command({'kind': 'task_action', 'task_id': tid, 'action': 'delete_files'})
            assert not output.exists()
            return {'elapsed_seconds': elapsed, 'cpu_seconds': cpu,
                    'rss_bytes': core.ps.memory_info().rss,
                    'io_delta': {field: getattr(io_after, field)-getattr(io_before, field)
                                 for field in ('read_count', 'write_count', 'read_bytes', 'write_bytes')},
                    'requests': requests, 'bytes': SIZE, 'sha256': actual,
                    'task_and_cache_removed': True}
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    args.root.mkdir(parents=True, exist_ok=False)
    engines = {'baseline-a': args.baseline, 'baseline-b': args.baseline, 'candidate': args.candidate}
    report = {'passed': False, 'scope': 'Core-only no-Range GET and POST; not full-product or Internet performance',
              'baseline_controls_identical': True, 'samples': [],
              'engine_sha256': {key: hashlib.sha256(value.read_bytes()).hexdigest()
                                for key, value in engines.items()}}

    def save():
        args.report.write_text(json.dumps(report, indent=2), encoding='utf-8')

    try:
        for round_index, order in enumerate((('baseline-a', 'baseline-b', 'candidate'),
                                            ('baseline-b', 'candidate', 'baseline-a'),
                                            ('candidate', 'baseline-a', 'baseline-b')), 1):
            for method, block_size in (('GET', 4096), ('GET', 65536), ('POST', 4096)):
                for label in order:
                    sample = {'round': round_index, 'label': label, 'method': method, 'server_block_bytes': block_size}
                    sample.update(transfer(engines[label], args.root/f'{round_index}-{method}-{block_size}-{label}',
                                           method, block_size))
                    report['samples'].append(sample)
                    save()
                    print(json.dumps(sample), flush=True)
        report['passed'] = True
        save()
    except Exception:
        report['error'] = traceback.format_exc()
        save()
        raise


if __name__ == '__main__':
    main()
