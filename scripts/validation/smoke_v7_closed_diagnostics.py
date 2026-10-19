"""A resident Core must remain usable after its launcher's stderr reader exits."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import threading
import uuid

from msi_checkpoint_fixture import BLOCK, PAYLOAD_SIZE, RangeHandler, Server, connect, send
from smoke_v7_portable_app import wait
from smoke_v7_transfer_performance import free_port


def run(args):
    report = {'passed': False, 'engine_sha256': hashlib.sha256(args.engine.read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    (Path.cwd() / '.tool-cache/test-tmp').mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='closed-diagnostics-', dir=Path.cwd() / '.tool-cache/test-tmp') as temporary:
        root = Path(temporary)
        port = free_port()
        env = {**os.environ, 'HLS_V7_DATA_DIR': str(root / 'data'),
               'HLS_V7_DOWNLOAD_DIR': str(root / 'downloads'),
               'HLS_V7_PIPE': rf'\\.\pipe\HLSDownloader.diagnostics-{uuid.uuid4().hex}',
               'HLS_V7_CORE_TCP': '1', 'HLS_V7_CORE_BIND': f'127.0.0.1:{port}'}
        process = subprocess.Popen([str(args.engine.resolve())], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                   creationflags=subprocess.CREATE_NO_WINDOW)
        class Handler(RangeHandler):
            def do_GET(self):
                if self.path == '/failure.bin':
                    self.send_error(404)
                else:
                    super().do_GET()
        server = Server(('127.0.0.1', 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            with connect(port) as ipc:
                before = send(ipc, {'type': 'load_settings', 'request_id': 1})
                assert before['type'] == 'settings', before
                # 在隔离数据库制造真实设置解码告警，不触及用户数据。
                with sqlite3.connect(root / 'data/data.db') as db:
                    db.execute('INSERT OR REPLACE INTO settings(key,value_json) VALUES (?,?)',
                               ('queue_profiles', json.dumps(before['queue_profiles'])))
                    db.execute('INSERT OR REPLACE INTO settings(key,value_json) VALUES (?,?)',
                               ('download_complete_popup_enabled', json.dumps('invalid-bool')))
                db.close()
                process.stderr.close()
                response = send(ipc, {'type': 'load_settings', 'request_id': 2})
                report['settings_response_type'] = response['type']
                assert response['type'] == 'settings', response
                response = send(ipc, {'type': 'snapshot', 'request_id': 3})
                report['snapshot_response_type'] = response['type']
                assert response['type'] == 'snapshot', response
                response = send(ipc, {'type':'store_settings','request_id':4,'values':{
                    'legal_terms_accepted':True,'legal_terms_version':'2026-08-06-cn-1',
                    'download_complete_popup_enabled':False,'download_progress_window_enabled':False,
                    'av_scan_enabled':False}})
                assert response.get('type') != 'error', response
                def create(filename):
                    response = send(ipc, {'type': 'command', 'request_id': 4,
                        'command': {'kind': 'create_task', 'spec': {
                            'url': f'http://127.0.0.1:{server.server_port}/{filename}',
                            'filename': filename, 'resource_kind': 'file',
                            'download_dir': str(root / 'downloads'),
                        }}})
                    assert response.get('type') != 'error', response
                    task_id = next(event['event']['snapshot']['task_id'] for event in response['events']
                        if event['event']['kind'] == 'task_created')
                    response = send(ipc, {'type':'command','request_id':4,'command':{
                        'kind':'task_action','task_id':task_id,'action':'start'}})
                    assert response.get('type') != 'error', response
                    return task_id
                def terminal(task_id):
                    response = send(ipc, {'type': 'snapshot', 'request_id': 5})
                    assert response['type'] == 'snapshot', response
                    task = next(task for task in response['tasks'] if task['task_id'] == task_id)
                    return task if task['status'] in ('failed', 'completed') else None
                failure_id = create('failure.bin')
                failed = wait(lambda: terminal(failure_id), 'HTTP failure not reported', 40)
                assert failed['status'] == 'failed' and failed['http_status'] == 404, failed
                report['http_failure_reported_without_poisoning'] = True
                success_id = create('success.bin')
                completed = wait(lambda: terminal(success_id), 'Download after failure did not finish', 60)
                assert completed['status'] == 'completed', completed
                output = Path(completed['output_path'])
                assert output.stat().st_size == PAYLOAD_SIZE
                assert hashlib.sha256(output.read_bytes()).digest() == hashlib.sha256(BLOCK * (PAYLOAD_SIZE // len(BLOCK))).digest()
                report['download_after_failure_verified'] = True
                report['passed'] = True
        except Exception as error:
            report['error'] = repr(error)
        finally:
            process.terminate()
            process.wait(timeout=10)
            server.shutdown()
            server.server_close()
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
    print(json.dumps(report))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
