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
import uuid

from msi_checkpoint_fixture import connect, send
from smoke_v7_transfer_performance import free_port


def run(args):
    report = {'passed': False, 'engine_sha256': hashlib.sha256(args.engine.read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='closed-diagnostics-', dir=Path.cwd() / '.tool-cache/test-tmp') as temporary:
        root = Path(temporary)
        port = free_port()
        env = {**os.environ, 'HLS_V7_DATA_DIR': str(root / 'data'),
               'HLS_V7_PIPE': rf'\\.\pipe\HLSDownloader.diagnostics-{uuid.uuid4().hex}',
               'HLS_V7_CORE_TCP': '1', 'HLS_V7_CORE_BIND': f'127.0.0.1:{port}'}
        process = subprocess.Popen([str(args.engine.resolve())], env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                   creationflags=subprocess.CREATE_NO_WINDOW)
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
                report['passed'] = True
        except Exception as error:
            report['error'] = repr(error)
        finally:
            process.terminate()
            process.wait(timeout=10)
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
    print(json.dumps(report))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
