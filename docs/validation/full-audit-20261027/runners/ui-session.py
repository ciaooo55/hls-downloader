"""Seed real tasks and observe UI state; native input is performed separately via sky."""
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import threading
import time
from urllib.request import Request, urlopen
import uuid

import psutil
from smoke_v7_portable_app import Pipe, wait

assert os.environ.get('HLS_VM_ACCEPTANCE') == '1'
runtime = Path(r'C:\QAWork\runtime')
reports = Path(r'C:\QAReports')
fixture = next(Path(r'C:\QAWork').glob('download-playback-*/fixtures'))
payload = bytes(range(256)) * 32768


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(fixture), **kwargs)

    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.path != '/slow.bin':
            return super().do_GET()
        self.send_response(200)
        self.send_header('Content-Length', str(len(payload)))
        self.end_headers()
        try:
            for offset in range(0, len(payload), 65536):
                self.wfile.write(payload[offset:offset + 65536])
                time.sleep(.03)
        except (BrokenPipeError, ConnectionResetError):
            pass


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()


def core(message):
    pipe = Pipe()
    try:
        return pipe.request(message)
    finally:
        pipe.close()


def command(value):
    return core({'type': 'command', 'request_id': 1, 'command': value})


def create(path, filename, **extra):
    result = command({'kind': 'create_task', 'spec': {
        'url': f'http://127.0.0.1:{server.server_port}/{path}',
        'filename': filename, 'resource_kind': 'file', 'concurrency': 1, **extra,
    }})
    assert 'events' in result, result
    return next(e['event']['snapshot']['task_id'] for e in result['events']
                if e['event']['kind'] == 'task_created')


media = create('source.mp4', '本轮真实媒体播放验收.mp4')
failed = create('missing.bin', '服务器拒绝的失败文件.bin')
paused = create('slow.bin', '暂停后可继续下载.bin')
wait(lambda: next((t for t in core({'type': 'snapshot', 'request_id': 2})['tasks']
                   if t['task_id'] == paused and t['downloaded_bytes'] > 0), None), 'No partial bytes')
command({'kind': 'task_action', 'task_id': paused, 'action': 'pause'})
scheduled = create('source.mp4', '超长标题与后缀布局检查-' + '示例说明' * 15 + '.mp4',
                   scheduled_start_at='2999-01-01T00:00:00Z', allow_duplicate=True)
wait(lambda: all(t['status'] in ('completed', 'failed', 'paused', 'queued') for t in
                  core({'type': 'snapshot', 'request_id': 3})['tasks']), 'Tasks still active')
token = uuid.uuid4().hex
port = 18761
env = {**os.environ, 'HLS_UI_TEST_API': '1', 'HLS_UI_TEST_PORT': str(port), 'HLS_UI_TEST_TOKEN': token}
log = (reports / 'ui-session.log').open('wb')
ui_process = subprocess.Popen([str(runtime / 'HLSDownloader.exe')], env=env, cwd=runtime,
                              stdout=log, stderr=log)


def state():
    try:
        request = Request(f'http://127.0.0.1:{port}/state', headers={'X-HLS-Test-Token': token})
        with urlopen(request, timeout=3) as response:
            return json.load(response)
    except OSError:
        return None


initial = wait(state, 'Workbench did not expose state', 90)
(reports / 'ui-session-state.json').write_text(json.dumps(initial, ensure_ascii=False, indent=2), 'utf8')
record = {'ready': True, 'port': port, 'token': token, 'pid': ui_process.pid,
          'tasks': {'media': media, 'failed': failed, 'paused': paused, 'scheduled': scheduled},
          'desktop_jar_sha256': hashlib.sha256(next((runtime / 'app').glob('HLSDownloaderDesktop-*.jar')).read_bytes()).hexdigest()}
(reports / 'ui-session-ready.json').write_text(json.dumps(record, indent=2), 'utf8')
while not (Path(r'C:\QAInput') / 'stop-ui-session').exists():
    time.sleep(1)
server.shutdown()
