"""Sandbox-only end-to-end transfer cost with normal and large HTTP headers."""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import statistics
import threading
import time
import uuid
from unittest.mock import patch

from compare_v7_sequential_io import transfer
from smoke_v7_resource_boundaries import Core


def probe(engine, root, padding):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            assert self.headers.get('Range') == 'bytes=0-0'
            self.send_response(206)
            self.send_header('Content-Range', 'bytes 0-0/1048576')
            self.send_header('Content-Length', '1')
            if padding:
                self.send_header('X-Fixture-Padding', 'x' * padding)
            self.end_headers()
            self.wfile.write(b'x')

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f'http://127.0.0.1:{server.server_port}'
    temporary = root/'probe-temp'
    temporary.mkdir(parents=True)
    try:
        with patch.dict(os.environ, {'TEMP': str(temporary), 'TMP': str(temporary)}), Core(engine, root) as core:
            before_files = set(temporary.rglob('*'))
            def request():
                response = core.command({'kind': 'harvest_page', 'url': origin+'/watch',
                                         'referer': origin+'/watch',
                                         'probe_urls': [origin+'/file.bin']})
                event = next(e['event'] for e in response['events']
                             if e['event']['kind'] == 'harvest_probe_result')
                assert len(event['links']) == 1 and event['links'][0]['size'] == 1048576, event

            for _ in range(3):
                request()
            cpu_before = sum(core.ps.cpu_times()[:2])
            started = time.perf_counter()
            for _ in range(20):
                request()
            return {'header_padding_bytes': padding, 'probes': 20,
                    'elapsed_seconds': time.perf_counter()-started,
                    'cpu_seconds': sum(core.ps.cpu_times()[:2])-cpu_before,
                    'rss_bytes': core.ps.memory_info().rss,
                    'temp_files_created': sum(path.is_file() and path not in before_files
                                              for path in temporary.rglob('*'))}
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--probe-only', action='store_true')
    mode.add_argument('--large-get-only', action='store_true')
    args = parser.parse_args()
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    engines = {'baseline-a': args.baseline, 'baseline-b': args.baseline,
               'candidate': args.candidate}
    result = {'passed': False, 'samples': [], 'medians': {},
              'scope': ('Core-only resource size probes; real IPC and SQLite commits' if args.probe_only else
                        'Core-only 128 MiB GET and POST without server Range support') +
                       '; loopback; not TLS, full-product or IDM/AB parity',
              'sha256': {name: hashlib.sha256(path.read_bytes()).hexdigest()
                         for name, path in engines.items()}}
    root = Path(r'C:\QAWork')/('http-headers-'+uuid.uuid4().hex)
    names = list(engines)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    try:
        methods = ('PROBE',) if args.probe_only else ('GET', 'POST')
        if args.large_get_only:
            methods = ('GET',)
        for method in methods:
            for padding in ((32768,) if args.large_get_only else (0, 32768)):
                key = f'{method}-{padding}'
                for number in range(3):
                    for name in names[number:] + names[:number]:
                        folder = root/f'{key}-{number}-{name}'
                        value = (probe(engines[name], folder, padding) if args.probe_only else
                                 transfer(engines[name], folder, method, 65536, header_bytes=padding))
                        if args.probe_only and name == 'candidate':
                            assert value['temp_files_created'] == 0, value
                        result['samples'].append({'variant': name, 'round': number+1,
                                                  'method': method, **value})
                        args.report.write_text(json.dumps(result, indent=2), encoding='utf-8')
                result['medians'][key] = {
                    name: {field: statistics.median(row[field] for row in result['samples']
                            if row['variant'] == name and row['method'] == method
                            and row['header_padding_bytes'] == padding)
                           for field in ('elapsed_seconds', 'cpu_seconds', 'rss_bytes')}
                    for name in names}
        result['passed'] = True
    finally:
        args.report.write_text(json.dumps(result, indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
