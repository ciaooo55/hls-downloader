"""Sandbox-only persisted replay, per-hop credentials and bounded 403 recovery."""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sqlite3
import threading
import uuid

from smoke_v7_resource_boundaries import Core, wait


def protect(value):
    class Blob(ctypes.Structure):
        _fields_ = [('size', wintypes.DWORD), ('data', ctypes.POINTER(ctypes.c_ubyte))]

    raw = value.encode()
    buffer = (ctypes.c_ubyte * len(raw)).from_buffer_copy(raw)
    source, output = Blob(len(raw), buffer), Blob()
    crypt = ctypes.WinDLL('crypt32', use_last_error=True)
    crypt.CryptProtectData.argtypes = [ctypes.POINTER(Blob), wintypes.LPCWSTR,
        ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(Blob)]
    crypt.CryptProtectData.restype = wintypes.BOOL
    if not crypt.CryptProtectData(ctypes.byref(source), None, None, None, None, 1,
                                 ctypes.byref(output)):
        raise ctypes.WinError(ctypes.get_last_error())
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.LocalFree.argtypes, kernel.LocalFree.restype = [ctypes.c_void_p], ctypes.c_void_p
    try:
        return 'dpapi:' + ctypes.string_at(output.data, output.size).hex()
    finally:
        kernel.LocalFree(output.data)


def run(engine, root):
    payload = b'persisted-browser-replay' * 4096
    requests, results = [], []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_HEAD(self):
            self.respond(False)

        def do_GET(self):
            self.respond(True)

        def respond(self, body):
            cookie, auth = self.headers.get('Cookie', ''), self.headers.get('Authorization', '')
            location = None
            if self.path == '/path/start':
                status, location = 302, '/target/file'
            elif self.path == '/cross':
                status, location = 302, destination + '/target/file'
            elif self.path == '/cross-unscoped':
                status, location = 302, destination + '/unscoped'
            elif self.path == '/unscoped':
                status = 200 if not cookie and not auth else 401
            elif self.path == '/good':
                status = 200
            else:
                session = 'target' if self.path.startswith('/target/') else 'ok'
                status = 200 if ('__cf_bm' not in cookie and '__cflb=pool-a' in cookie
                    and f'session={session}' in cookie and auth == f'Bearer {session}') else 403
            # 仅记录测试凭据是否正确，报告不保存请求身份值。
            requests.append({'method': self.command, 'path': self.path,
                'port': self.server.server_port, 'status': status,
                'stale_cookie': '__cf_bm' in cookie, 'has_cookie': bool(cookie),
                'has_authorization': bool(auth)})
            self.send_response(status)
            if location:
                self.send_header('Location', location)
            self.send_header('Content-Length', str(len(payload) if status == 200 else 0))
            self.send_header('Connection', 'close')
            self.end_headers()
            if body and status == 200:
                self.wfile.write(payload)

    servers = [ThreadingHTTPServer(('127.0.0.1', 0), Handler) for _ in range(2)]
    source, destination = [f'http://127.0.0.1:{server.server_port}' for server in servers]
    for server in servers:
        server.daemon_threads = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
    cookie = '__cf_bm=stale; __cflb=pool-a; session=ok'
    target_cookie = cookie.replace('session=ok', 'session=target')
    cases = {
        'cookie': ('/file', {'cookie': cookie, 'request_headers': {'Authorization': 'Bearer ok'}}),
        'request_headers': ('/file', {'request_headers': {'Cookie': cookie, 'Authorization': 'Bearer ok'}}),
        'scoped': ('/file', {'request_contexts': {source: {'cookie': cookie,
            'request_headers': {'Authorization': 'Bearer ok'}}}}),
        'cookie_jar': ('/file', {'request_contexts': {source: {'cookies': [
            {'name': name, 'value': value, 'domain': '127.0.0.1', 'path': '/'}
            for name, value in [('__cf_bm', 'stale'), ('__cflb', 'pool-a'), ('session', 'ok')]],
            'request_headers': {'Authorization': 'Bearer ok'}}}}),
        'path_redirect': ('/path/start', {'request_contexts': {source: {'cookies': [
            {'name': name, 'value': value, 'domain': '127.0.0.1', 'path': path}
            for path, session in [('/path/', 'ok'), ('/target/', 'target')]
            for name, value in [('__cf_bm', 'stale'), ('__cflb', 'pool-a'), ('session', session)]],
            'path_contexts': {
                '/path/': {'request_headers': {'Authorization': 'Bearer ok'}},
                '/target/': {'request_headers': {'Authorization': 'Bearer target'}}}}}}),
        'scoped_cross_origin': ('/cross', {'cookie': cookie,
            'request_headers': {'Authorization': 'Bearer ok'}, 'request_contexts': {
                destination: {'cookie': target_cookie,
                    'request_headers': {'Authorization': 'Bearer target'}}}}),
        'unscoped_cross_origin': ('/cross-unscoped', {'cookie': cookie,
            'request_headers': {'Authorization': 'Bearer ok'}}),
    }
    blobs = {}
    try:
        with Core(engine, root) as core:
            for name, (_, context) in cases.items():
                blobs[name] = protect(json.dumps(context))
                core.call({'type': 'store_credential', 'request_id': 11,
                    'credential_ref': name, 'protected_blob': blobs[name], 'kind': 'browser_replay'})
        with sqlite3.connect(root/'data/data.db') as db:
            saved = dict(db.execute('SELECT credential_ref, protected_blob FROM credentials'))
        assert all(saved[name] == blob and blob.startswith('dpapi:') for name, blob in blobs.items())
        with Core(engine, root) as core:
            core.call({'type': 'store_settings', 'request_id': 12, 'values': {'queue_max_active': 1}})

            def download(name, path, credential=None, failed=False):
                start = len(requests)
                spec = {'url': source + path, 'filename': name + '.bin', 'resource_kind': 'file',
                        'proxy': 'hls-downloader://direct-proxy'}
                if credential:
                    spec['credential_ref'] = credential
                reply = core.command({'kind': 'create_task', 'spec': spec})
                tid = next(e['event']['snapshot']['task_id'] for e in reply['events']
                           if e['event']['kind'] == 'task_created')
                def terminal():
                    task = next(t for t in core.snapshot()['tasks'] if t['task_id'] == tid)
                    return task if task['status'] in ('completed', 'failed') else None
                task = wait(terminal, 30)
                assert task['status'] == ('failed' if failed else 'completed'), task
                output = Path(task['output_path']) if task['output_path'] else None
                if not failed:
                    assert output.read_bytes() == payload
                    assert not (root/'downloads/.hls-tasks'/tid).exists()
                recorded = requests[start:]
                assert recorded and len(recorded) < 20, recorded
                if credential and name != 'unscoped_cross_origin':
                    assert any(r['status'] == 403 and r['stale_cookie'] for r in recorded), recorded
                    assert any(r['status'] == 200 and not r['stale_cookie'] for r in recorded), recorded
                core.command({'kind': 'task_action', 'task_id': tid, 'action': 'delete_files'})
                wait(lambda: not (root/'downloads/.hls-tasks'/tid).exists())
                assert not output or not output.exists()
                results.append({'case': name, 'expected_status': task['status'],
                    'requests': len(recorded), 'output_and_cache_cleaned': True})

            for name, (path, _) in cases.items():
                download(name, path, name)
            download('permanent_403', '/denied', failed=True)
            download('following_queue', '/good')
        with sqlite3.connect(root/'data/data.db') as db:
            unchanged = dict(db.execute('SELECT credential_ref, protected_blob FROM credentials'))
        assert unchanged == saved, 'Retry must not mutate persisted credentials'
        return {'cases': results, 'requests': requests, 'persisted_dpapi_after_restart': True,
            'saved_replay_unchanged': True, 'payload_sha256': hashlib.sha256(payload).hexdigest()}
    finally:
        for server in servers:
            server.shutdown()
            server.server_close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    result = run(args.engine, Path(r'C:\QAWork')/('replay-retry-' + uuid.uuid4().hex))
    result.update(passed=True, engine_sha256=hashlib.sha256(args.engine.read_bytes()).hexdigest())
    args.report.write_text(json.dumps(result, indent=2), encoding='utf-8')
