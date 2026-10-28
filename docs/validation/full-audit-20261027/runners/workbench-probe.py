"""Installed workbench rendering and settings persistence; no native input."""
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import re
import subprocess
import threading
import time
from urllib.request import Request, urlopen
import uuid

from PIL import Image
from smoke_v7_portable_app import Pipe, wait

assert os.environ.get('HLS_VM_ACCEPTANCE') == '1'
runtime = Path(r'C:\QA Workbench')
root = Path(r'C:\QAWork\workbench')
root.mkdir(parents=True, exist_ok=True)
reports = Path(r'C:\QAReports')
fixtures = root/'fixtures'
fixtures.mkdir(exist_ok=True)
payload = bytes(range(256))*4096
(fixtures/'sample.bin').write_bytes(payload)
subprocess.run([str(runtime/'app/resources/ffmpeg.exe'), '-hide_banner', '-loglevel', 'error',
                '-f', 'lavfi', '-i', 'color=c=blue:s=320x180:r=24', '-t', '1',
                '-c:v', 'mpeg4', str(fixtures/'sample.mp4')], check=True, creationflags=subprocess.CREATE_NO_WINDOW)


class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(fixtures), **kwargs)

    def log_message(self, *_):
        pass


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
env = {**os.environ, 'HLS_V7_DATA_DIR': str(root/'data'),
       'HLS_V7_DOWNLOAD_DIR': str(root/'downloads'), 'HLS_UI_TEST_API': '1',
       'HLS_UI_TEST_PORT': '18761', 'HLS_UI_TEST_TOKEN': uuid.uuid4().hex}
os.environ.update({key: env[key] for key in ('HLS_V7_DATA_DIR', 'HLS_V7_DOWNLOAD_DIR')})
config = runtime/'app/HLSDownloader.cfg'
original = config.read_text('utf8')
processes = []
result = {'passed': False, 'scope': 'Installed component hashes, real local downloads, Core settings persistence and paint capture. No native mouse/keyboard or drag/button acceptance.', 'captures': []}


def core(message):
    pipe = Pipe()
    try:
        return pipe.request(message)
    finally:
        pipe.close()


def start_core():
    log = (reports/'workbench-core.txt').open('ab')
    proc = subprocess.Popen([str(runtime/'app/resources/HLSDownloaderEngine.exe')], env=env,
                            stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
    processes.append(proc)
    def ready():
        try:
            return core({'type': 'snapshot', 'request_id': 1})
        except OSError:
            return None
    wait(ready, 'Core startup', 30)
    return proc


def ui(endpoint, raw=False, body=None):
    req = Request('http://127.0.0.1:18761/'+endpoint,
                  headers={'X-HLS-Test-Token': env['HLS_UI_TEST_TOKEN'], 'Content-Type': 'application/json'},
                  data=None if body is None else json.dumps(body).encode())
    with urlopen(req, timeout=10) as response:
        data = response.read()
    return data if raw else json.loads(data)


def stop(proc):
    if proc.poll() is None:
        proc.terminate()
        proc.wait(15)


layout = 'name:420:1,size:110:1,progress:180:1,status:100:1,speed:85:0,actions:70:1'
try:
    engine = start_core()
    core({'type': 'store_settings', 'request_id': 1, 'values': {
        'legal_terms_accepted': True, 'legal_terms_version': '2026-08-06-cn-1',
        'av_scan_enabled': False, 'download_progress_window_enabled': False,
        'download_complete_popup_enabled': False}})
    ids = []
    for name, extra in (('sample.bin', {}), ('sample.mp4', {}),
                        ('queued.bin', {'scheduled_start_at': '2999-01-01T00:00:00Z', 'allow_duplicate': True})):
        response = core({'type': 'command', 'request_id': 2, 'command': {'kind': 'create_task', 'spec': {
            'url': f'http://127.0.0.1:{server.server_port}/'+('sample.bin' if name == 'queued.bin' else name),
            'filename': name, 'resource_kind': 'file', 'concurrency': 1,
            'proxy': 'hls-downloader://direct-proxy', **extra}}})
        created = [event['event']['snapshot']['task_id'] for event in response['events']
                   if event['event']['kind'] == 'task_created']
        assert len(created) == 1, response
        ids.append(created[0])
    def downloads_complete():
        items = core({'type': 'snapshot', 'request_id': 3})['tasks']
        return items if sum(t['status'] == 'completed' for t in items) == 2 else None
    tasks = wait(downloads_complete, 'Actual fixture downloads did not complete', 60)
    completed = next(t for t in tasks if t['task_id'] == ids[0])
    assert hashlib.sha256(Path(completed['output_path']).read_bytes()).digest() == hashlib.sha256(payload).digest()
    core({'type': 'store_settings', 'request_id': 4, 'values': {'task_column_layout': layout, 'dark_mode': False}})
    stop(engine)
    engine = start_core()
    settings = core({'type': 'load_settings', 'request_id': 5})
    assert settings['task_column_layout'] == layout, settings
    result['layout_survives_core_restart'] = True
    result['real_download_sha256'] = hashlib.sha256(payload).hexdigest()
    for label, width, scale, dark in (('light-wide', 1400, '1.0', False), ('dark-compact-150', 1024, '1.5', True)):
        core({'type': 'store_settings', 'request_id': 6, 'values': {'dark_mode': dark}})
        configured = re.sub(r'java-options=-Dsun\.java2d\.uiScale=[^\r\n]+\r?\n?', '', original)
        config.write_text(configured+'\njava-options=-Dsun.java2d.uiScale='+scale+'\n', 'utf8')
        log = (reports/f'workbench-{label}.txt').open('wb')
        proc = subprocess.Popen([str(runtime/'HLSDownloader.exe')], env={**env,
            'HLS_UI_AUDIT_WIDTH': str(width), 'HLS_UI_AUDIT_HEIGHT': '820'},
            cwd=runtime, stdout=log, stderr=log)
        processes.append(proc)

        def connected():
            try:
                state = ui('state')
                return state if state.get('controlBounds') and '已连接' in state.get('engineText', '') else None
            except OSError:
                return None

        wait(connected, 'Installed workbench did not connect', 90)
        ui('action', body={'type': 'select_task', 'index': 0})
        wait(lambda: ui('state')['selectedCount'] == 1, 'Internal selection did not update')

        def capture():
            data = ui('screenshot?mode=paint', raw=True)
            (reports/f'workbench-{label}.png').write_bytes(data)
            with Image.open(io.BytesIO(data)) as image:
                counts = sorted((n for n, _ in image.convert('RGB').getcolors(image.width*image.height)), reverse=True)
                return image.width*image.height-sum(counts[:2]) > 500

        wait(capture, 'Paint capture stayed blank', 20)
        result['captures'].append({'label': label, 'window': ui('window'), 'state': ui('state')})
        stop(proc)
    core({'type': 'store_settings', 'request_id': 7, 'values': {'task_column_layout': ''}})
    stop(engine)
    engine = start_core()
    assert core({'type': 'load_settings', 'request_id': 8})['task_column_layout'] == ''
    result['default_layout_reset_survives_restart'] = True
    result['passed'] = True
finally:
    for proc in reversed(processes):
        stop(proc)
    config.write_text(original, 'utf8')
    server.shutdown()
    server.server_close()
    (reports/'workbench-probe.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), 'utf8')
