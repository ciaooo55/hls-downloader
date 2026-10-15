"""Guest-only regression of HTTP framing and archive publication."""
from __future__ import annotations
import argparse
import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import traceback
import uuid
import zipfile
from msi_checkpoint_fixture import connect, send, wait_task
from smoke_v7_transfer_performance import free_port


def run(args):
    report = {'passed': False, 'checks': [], 'engine_sha256': hashlib.sha256(args.engine.read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    cache = Path.cwd()/'.tool-cache/test-tmp'
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='stream-archive-', dir=cache) as tmp:
        root = Path(tmp)
        payload = bytes(range(256))*8192
        archive = io.BytesIO()
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as z:
            z.writestr('manifest.json', '{"manifest_version":3,"name":"QA","version":"1.0"}')
        zipped = archive.getvalue()
        counts = {}
        class Handler(BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'
            def log_message(self, *_): pass
            def do_GET(self): self.respond()
            def do_POST(self): self.respond()
            def respond(self):
                route = self.path.split('?')[0]
                counts[route] = counts.get(route, 0)+1
                if self.command == 'POST':
                    assert self.rfile.read(int(self.headers.get('Content-Length', '0'))) == b'fixture=ok'
                self.send_response(200)
                self.send_header('Content-Type', 'application/octet-stream')
                self.send_header('Connection', 'close')
                if route == '/quoted':
                    self.send_header('Content-Disposition', 'attachment; filename="report;2026.csv"')
                if route == '/server-xpi':
                    self.send_header('Content-Disposition', 'attachment; filename="extension.xpi"')
                body = zipped if 'archive' in route or 'xpi' in route else b'a,b\n1,2\n' if route == '/quoted' else payload
                if route.startswith('/chunk'):
                    self.send_header('Transfer-Encoding', 'chunked')
                    if route == '/chunk-conflicting-length': self.send_header('Content-Length', '1')
                else: self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.close_connection = True
                try:
                    if route == '/chunk-bad-size': self.wfile.write(b'xyz\r\n'); return
                    if route == '/chunk-overflow': self.wfile.write(b'FFFFFFFFFFFFFFFFF\r\n'); return
                    if route == '/chunk-bad-crlf': self.wfile.write(b'1\r\nx\n0\r\n\r\n'); return
                    if route.startswith('/chunk'):
                        for offset in range(0, len(body), 16384):
                            chunk = body[offset:offset+16384]
                            self.wfile.write(f'{len(chunk):X};qa="a;b"\r\n'.encode()+chunk+b'\r\n')
                            self.wfile.flush()
                            if route == '/chunk-slow': time.sleep(.025)
                        if route != '/chunk-truncated': self.wfile.write(b'0;done=1\r\nX-QA: completed\r\n\r\n')
                    else: self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError): pass
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        port = free_port()
        env = {**os.environ, 'HLS_V7_DATA_DIR': str(root/'data'), 'HLS_V7_DOWNLOAD_DIR': str(root/'downloads'),
               'HLS_V7_PIPE': rf'\\.\pipe\HLSDownloader.archive-{uuid.uuid4().hex}',
               'HLS_V7_CORE_TCP': '1', 'HLS_V7_CORE_BIND': f'127.0.0.1:{port}'}
        process = None
        def check(name, fn):
            try:
                detail = fn()
                report['checks'].append({'name': name, 'passed': True, 'detail': detail})
                print(name+' passed', flush=True)
            except Exception:
                report['checks'].append({'name': name, 'passed': False, 'error': traceback.format_exc()})
                print(name+' FAILED', flush=True)
        try:
            with args.report.with_suffix('.core.log').open('wb') as log:
                process = subprocess.Popen([str(args.engine.resolve())], env=env, stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
            with connect(port) as ipc:
                def command(value):
                    r = send(ipc, {'type': 'command', 'request_id': 2, 'command': value})
                    assert r.get('type') != 'error', r
                    return r
                send(ipc, {'type': 'store_settings', 'request_id': 1, 'values': {'legal_terms_accepted': True, 'legal_terms_version': '2026-08-06-cn-1', 'download_complete_popup_enabled': False, 'download_progress_window_enabled': False, 'av_scan_enabled': False}})
                def create(route, filename, **extras):
                    r = command({'kind': 'create_task', 'spec': {'url': f'http://127.0.0.1:{server.server_port}{route}?id={uuid.uuid4().hex}', 'filename': filename, 'resource_kind': 'file', 'concurrency': 1, 'allow_duplicate': True, 'proxy': 'hls-downloader://direct-proxy', **extras}})
                    return next(e['event']['snapshot']['task_id'] for e in r['events'] if e['event']['kind'] == 'task_created')
                def finished(tid): return wait_task(ipc, tid, lambda t: t['status'] in ('completed', 'failed'), 60)
                def download(route, filename, expected, suffix, **extras):
                    task = finished(create(route, filename, **extras))
                    assert task['status'] == 'completed', task
                    path = Path(task['output_path'])
                    assert path.suffix == suffix, (str(path), suffix)
                    assert path.name == task['filename'], task
                    assert hashlib.sha256(path.read_bytes()).digest() == hashlib.sha256(expected).digest(), 'payload changed'
                    return {'filename': path.name, 'requests': counts.get(route), 'sha256': hashlib.sha256(expected).hexdigest()}
                for ext in ('xpi', 'whl', 'vsix', 'nupkg', 'msix', 'appx', 'cbz', 'docx'):
                    check('preserve-zip-container-'+ext, lambda ext=ext: download('/archive-'+ext, 'package.'+ext, zipped, '.'+ext))
                check('server-supplied-xpi-extension', lambda: download('/server-xpi', 'package.php', zipped, '.xpi'))
                check('quoted-semicolon-filename', lambda: download('/quoted', 'report.php', b'a,b\n1,2\n', '.csv'))
                check('chunked-extensions-trailers-sha256', lambda: download('/chunk', 'stream.bin', payload, '.bin'))
                check('chunked-ignores-conflicting-content-length', lambda: download('/chunk-conflicting-length', 'stream.bin', payload, '.bin'))
                check('system-proxy-chunked-conflicting-length', lambda: download('/chunk-conflicting-length', 'system.bin', payload, '.bin', proxy='hls-downloader://system-proxy'))
                post = root/'post'; post.write_bytes(b'fixture=ok')
                def one_post():
                    detail = download('/chunk-post', 'post.bin', payload, '.bin', request_method='POST', body_path=str(post))
                    assert counts['/chunk-post'] == 1, counts
                    return detail
                check('chunked-post-only-once', one_post)
                for route in ('/chunk-truncated', '/chunk-bad-size', '/chunk-overflow', '/chunk-bad-crlf'):
                    def fails(route=route):
                        task = finished(create(route, route[1:]+'.bin'))
                        assert task['status'] == 'failed' and task.get('error_message'), task
                        assert not task.get('output_path') or not Path(task['output_path']).is_file(), task
                        return task['error_message']
                    check(route[1:]+'-never-published', fails)
                def resume():
                    tid = create('/chunk-slow', 'paused.bin')
                    wait_task(ipc, tid, lambda t: t['downloaded_bytes'] >= 131072, 60)
                    command({'kind': 'task_action', 'task_id': tid, 'action': 'pause'})
                    wait_task(ipc, tid, lambda t: t['status'] == 'paused' and t['active_workers'] == 0, 60)
                    command({'kind': 'task_action', 'task_id': tid, 'action': 'resume'})
                    task = finished(tid)
                    assert task['status'] == 'completed', task
                    assert Path(task['output_path']).read_bytes() == payload
                    return {'requests': counts['/chunk-slow']}
                check('chunked-pause-resume-sha256', resume)
                def collisions():
                    send(ipc, {'type': 'store_settings', 'request_id': 1, 'values': {'existing_file_policy': 'rename'}})
                    first = finished(create('/archive-collision', 'same.xpi'))
                    second = finished(create('/archive-collision', 'same.xpi'))
                    assert first['status'] == second['status'] == 'completed', (first, second)
                    a, b = Path(first['output_path']), Path(second['output_path'])
                    assert a != b and a.suffix == b.suffix == '.xpi' and a.read_bytes() == b.read_bytes() == zipped
                    a.write_bytes(b'preserve-existing')
                    send(ipc, {'type': 'store_settings', 'request_id': 1, 'values': {'existing_file_policy': 'skip'}})
                    skipped = finished(create('/archive-collision', 'same.xpi'))
                    assert skipped['status'] == 'failed' and a.read_bytes() == b'preserve-existing', skipped
                    send(ipc, {'type': 'store_settings', 'request_id': 1, 'values': {'existing_file_policy': 'overwrite'}})
                    replaced = finished(create('/archive-collision', 'same.xpi'))
                    assert replaced['status'] == 'completed' and Path(replaced['output_path']) == a and a.read_bytes() == zipped, replaced
                    assert b.read_bytes() == zipped
                    return {'rename': [a.name, b.name], 'skip_preserved': True, 'overwrite': a.name}
                check('corrected-extension-rename-skip-overwrite-preserves-files', collisions)
            report['passed'] = all(c['passed'] for c in report['checks'])
        except Exception: report['error'] = traceback.format_exc()
        finally:
            if process:
                process.terminate()
                with contextlib.suppress(subprocess.TimeoutExpired): process.wait(timeout=10)
            server.shutdown(); server.server_close()
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
