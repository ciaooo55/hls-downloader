"""Sandbox-only malformed/oversized HTTP ranges and queue continuation."""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import threading
import uuid

from smoke_v7_resource_boundaries import Core, wait


def run(engine, root):
    payload = b'range-response-fixture' * 4096
    invalid = {
        'utf8': 'byte界 0-7/8'.encode('utf-8'),
        'invalid-utf8': b'byte\xff 0-7/8',
        'ascii': b'bytes invalid-7/8',
    }
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            name = self.path.strip('/')
            requests.append({'path': self.path, 'range': self.headers.get('Range')})
            if name.endswith('/playlist.m3u8'):
                manifest = (b'#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n'
                            b'#EXTINF:1,\n#EXT-X-BYTERANGE:8@0\nsegment.bin\n#EXT-X-ENDLIST\n')
                self.send_response(200)
                self.send_header('Content-Length', str(len(manifest)))
                self.end_headers()
                self.wfile.write(manifest)
                return
            if name.endswith('/segment.bin'):
                chunked = name.startswith('oversize-chunked/')
                framing = b'Transfer-Encoding: chunked' if chunked else b'Content-Length: 64'
                # 只有客户端主动放弃错误响应才结束，避免正常 EOF 掩盖无界读取。
                self.connection.settimeout(35)
                try:
                    self.connection.sendall(b'HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-7/8\r\n' +
                        framing + b'\r\nConnection: close\r\n\r\n' +
                        (b'9\r\n123456789\r\n' if chunked else b'123456789'))
                    closed = self.connection.recv(1) == b''
                except (ConnectionResetError, BrokenPipeError):
                    closed = True
                except TimeoutError:
                    closed = False
                requests.append({'path': self.path, 'client_closed_before_eof': closed})
                self.close_connection = True
                return
            if name in invalid:
                self.connection.sendall(b'HTTP/1.1 206 Partial Content\r\nContent-Range: ' +
                    invalid[name] + b'\r\nContent-Length: 8\r\nConnection: close\r\n\r\n01234567')
                self.close_connection = True
                return
            start, end = 0, len(payload) - 1
            value = self.headers.get('Range')
            if value:
                start_text, end_text = value.removeprefix('bytes=').split('-')
                start, end = int(start_text), int(end_text) if end_text else end
            self.send_response(206 if value else 200)
            if value:
                self.send_header('Content-Range', f'bytes {start}-{end}/{len(payload)}')
            self.send_header('Content-Length', str(end - start + 1))
            self.send_header('ETag', '"range-fixture"')
            self.send_header('Connection', 'close')
            self.end_headers()
            self.wfile.write(payload[start:end + 1])

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    results = []
    try:
        with Core(engine, root) as core:
            core.call({'type': 'store_settings', 'request_id': 10,
                       'values': {'queue_max_active': 1}})

            def create(path, filename, resource_kind='file'):
                reply = core.command({'kind': 'create_task', 'spec': {
                    'url': f'http://127.0.0.1:{server.server_port}/{path}',
                    'filename': filename, 'resource_kind': resource_kind,
                    'proxy': 'hls-downloader://direct-proxy'}})
                return next(e['event']['snapshot']['task_id'] for e in reply['events']
                            if e['event']['kind'] == 'task_created')

            for name in [*invalid, 'oversize-length', 'oversize-chunked']:
                oversized = name.startswith('oversize-')
                bad = create(name + '/playlist.m3u8' if oversized else name,
                             name + '.bin', 'hls' if oversized else 'file')
                good = create('good', name + '-following.bin')

                def terminal():
                    tasks = {t['task_id']: t for t in core.snapshot()['tasks']}
                    return tasks if all(tasks[tid]['status'] in ('completed', 'failed')
                                        for tid in (bad, good)) else None

                tasks = wait(terminal, 30)
                assert tasks[bad]['status'] == 'failed', tasks[bad]
                if oversized:
                    assert 'byte range' in json.dumps(tasks[bad]), tasks[bad]
                    wait(lambda: any(r.get('client_closed_before_eof')
                                     for r in requests if r['path'] == '/' + name + '/segment.bin'))
                assert tasks[good]['status'] == 'completed', tasks[good]
                output = Path(tasks[good]['output_path'])
                assert output.read_bytes() == payload
                for tid in (bad, good):
                    core.command({'kind': 'task_action', 'task_id': tid, 'action': 'delete_files'})
                    wait(lambda: not (root/'downloads/.hls-tasks'/tid).exists())
                assert not output.exists()
                results.append({'case': name, 'fault_task_failed': True,
                                'following_task_sha256': hashlib.sha256(payload).hexdigest(),
                                'tasks_files_and_caches_removed': True})
        assert 'panicked at' not in (root/'core.log').read_text(encoding='utf-8', errors='replace')
        return {'cases': results, 'requests': requests, 'no_core_panic': True}
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    result = run(args.engine, Path(r'C:\QAWork')/('range-faults-' + uuid.uuid4().hex))
    result.update(passed=True, engine_sha256=hashlib.sha256(args.engine.read_bytes()).hexdigest())
    args.report.write_text(json.dumps(result, indent=2), encoding='utf-8')
