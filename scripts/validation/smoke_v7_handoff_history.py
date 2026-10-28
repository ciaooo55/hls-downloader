"""Sandbox-only Native Host/IPC check with history larger than one legal frame."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import sqlite3
import struct
import subprocess
import time
import traceback
import uuid

from msi_checkpoint_fixture import connect, send


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Windows Sandbox only'
    root = Path(r'C:\QAWork')/('handoff-history-'+uuid.uuid4().hex[:8])
    data = root/'data'
    data.mkdir(parents=True)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'expected_legacy_failure': args.expect_legacy_failure,
              'expected_pending_overflow': args.expect_pending_overflow,
              'pending_history': args.pending_history, 'paged': args.paged,
              'engine_sha256': hashlib.sha256((args.runtime/'HLSDownloaderEngine.exe').read_bytes()).hexdigest(),
              'native_host_sha256': hashlib.sha256((args.runtime/'HLSDownloaderNativeHost.exe').read_bytes()).hexdigest()}
    row = {'id': '', 'offer': {'url': 'https://cdn.test/history.bin', 'resource_kind': 'file'},
           'filename': 'history.bin', 'title': 'x'*(384*1024), 'mime_type': '', 'size': 1,
           'status': 'pending' if args.pending_history else 'accepted', 'presentation': 'presented', 'task_id': None,
           'created_at_ms': 1, 'request_id': ''}
    with sqlite3.connect(data/'data.db') as db:
        db.execute('CREATE TABLE handoffs (handoff_id TEXT PRIMARY KEY, public_json TEXT NOT NULL, '
                   'status TEXT NOT NULL, task_id TEXT, created_at_ms INTEGER NOT NULL)')
        for index in range(12):
            row.update(id=f'history-{index}', request_id=f'client-{index}')
            db.execute('INSERT INTO handoffs VALUES (?, ?, ?, NULL, ?)',
                       (row['id'], json.dumps(row), row['status'], 1))
        report['history_json_bytes'] = db.execute('SELECT sum(length(public_json)) FROM handoffs').fetchone()[0]
        assert report['history_json_bytes'] > 4*1024*1024
        row.update(id='pending-one', title='', request_id='pending-client', status='pending')
        db.execute('INSERT INTO handoffs VALUES (?, ?, ?, NULL, ?)',
                   (row['id'], json.dumps(row), row['status'], 2))
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    env = {**os.environ, 'HLS_V7_CORE_BIND': f'127.0.0.1:{port}', 'HLS_V7_CORE_TCP': '1',
           'HLS_V7_DATA_DIR': str(data), 'HLS_V7_DOWNLOAD_DIR': str(root/'downloads')}
    process = None
    try:
        with args.report.with_suffix('.runtime.txt').open('wb') as log:
            process = subprocess.Popen([str(args.runtime/'HLSDownloaderEngine.exe')],
                                       env=env, cwd=args.runtime, stdout=log, stderr=log)
            with connect(port):
                pass
            requests = [{'op': 'ping'}, {'op': 'handoff_status', 'handoff_id': 'pending-one'}]
            payload = b''
            for request in requests:
                body = json.dumps(request).encode()
                payload += struct.pack('<I', len(body))+body
            started = time.monotonic()
            host = subprocess.run([str(args.runtime/'HLSDownloaderNativeHost.exe')],
                                  input=payload, capture_output=True, env=env, timeout=20)
            report['native_host'] = {'exit_code': host.returncode, 'stdout_bytes': len(host.stdout),
                                     'elapsed_ms': (time.monotonic()-started)*1000,
                                     'stderr': host.stderr.decode('utf8', 'replace')[:1500]}
            decoded = []
            raw = host.stdout
            while raw:
                size = struct.unpack('<I', raw[:4])[0]
                decoded.append(json.loads(raw[4:4+size]))
                raw = raw[4+size:]
            report['native_host']['responses'] = decoded
            row.update(id='saved-after-history', request_id='saved-client')
            encoded = json.dumps(row)
            try:
                with connect(port) as ipc:
                    saved = send(ipc, {'type': 'save_handoff', 'request_id': 1,
                                      'handoff_id': row['id'], 'handoff_json': encoded,
                                      'status': 'pending', 'task_id': None, 'created_at_ms': 3})
                report['save'] = {'ok': saved.get('type') == 'handoffs',
                                  'response_bytes': len(json.dumps(saved).encode())}
            except (OSError, RuntimeError) as error:
                report['save'] = {'ok': False, 'error': str(error)}
            with sqlite3.connect(data/'data.db') as db:
                report['row_committed'] = db.execute('SELECT count(*) FROM handoffs WHERE handoff_id=?',
                                                     (row['id'],)).fetchone()[0] == 1
            assert report['row_committed'], 'Fixture/save did not commit the new row'
            if args.expect_legacy_failure:
                assert host.returncode != 0 and not decoded and not report['save']['ok'], report
                report['legacy_failure_reproduced'] = True
            else:
                assert host.returncode == 0 and len(decoded) == 2 and all(r.get('ok') is True for r in decoded), report
                assert decoded[1]['handoff']['id'] == 'pending-one' and report['save']['ok'], report
                with connect(port) as ipc:
                    pending_ids = ([f'history-{index}' for index in range(12)] if args.pending_history else [])
                    for filters, expected in (({'handoff_id': row['id']}, [row['id']]),
                                              ({'client_request_id': 'client-7'}, ['history-7']),
                                              ({'pending_only': True}, pending_ids+['pending-one', row['id']])):
                        actual = []
                        after = None
                        pages = 0
                        while True:
                            response = send(ipc, {'type': 'load_handoffs', 'request_id': 2,
                                                  'paged': args.paged, 'after': after, **filters})
                            assert len(json.dumps(response).encode()) <= 4*1024*1024
                            actual.extend(json.loads(item)['id'] for item in response['items'])
                            pages += 1
                            cursor = response.get('next')
                            if cursor is None:
                                break
                            assert after is None or cursor > after, cursor
                            after = cursor
                        assert actual == expected, (filters, actual)
                        if filters == {'pending_only': True}:
                            report['pending_page_count'] = pages
                            if args.pending_history and args.paged:
                                assert pages > 1
                report['filtered_reads_passed'] = True
            report['passed'] = not args.expect_legacy_failure
    except Exception:
        report['error'] = traceback.format_exc()
        if args.expect_pending_overflow:
            assert args.pending_history and not args.paged
            assert report.get('row_committed') and report.get('save', {}).get('ok')
            assert 'Core IPC closed' in report['error'], report['error']
            report['legacy_pending_overflow_reproduced'] = True
    finally:
        if process:
            try:
                with connect(port) as ipc:
                    send(ipc, {'type': 'command', 'request_id': 3, 'command': {'kind': 'shutdown'}})
                process.wait(timeout=10)
            except Exception:
                process.kill()
                process.wait(timeout=5)
            report['core_exit_code'] = process.returncode
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n', encoding='utf8')
    return 0 if report.get('legacy_failure_reproduced') or report.get('legacy_pending_overflow_reproduced') or report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--expect-legacy-failure', action='store_true')
    parser.add_argument('--pending-history', action='store_true')
    parser.add_argument('--paged', action='store_true')
    parser.add_argument('--expect-pending-overflow', action='store_true')
    raise SystemExit(run(parser.parse_args()))
