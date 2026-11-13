"""Bounded wall-clock soak: 1000 persisted tasks plus repeated real downloads."""
from __future__ import annotations

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import random
import socket
import threading
import time
import traceback
import uuid

from soak_v7_runtime import percentile
from validation_workspace import test_tmp


def resource_growth(samples):
    results = []
    for pid in sorted({s['pid'] for s in samples}):
        process = [s for s in samples if s['pid'] == pid]
        idle = [s for s in process if s['state'] == 'idle']
        assert len(idle) >= 2, f'No comparable idle checkpoints for PID {pid}'
        result = {'pid': pid, 'idle_checkpoints': len(idle),
                  'initial_idle_handles': idle[0]['handles'],
                  'final_idle_handles': idle[-1]['handles'],
                  'peak_idle_handle_growth': max(s['handles'] for s in idle)-idle[0]['handles'],
                  'peak_handles': max(s['handles'] for s in process),
                  'peak_rss_growth_mib': max(s['rss_mib'] for s in process)-process[0]['rss_mib']}
        assert result['peak_rss_growth_mib'] <= 256, result
        assert result['peak_idle_handle_growth'] <= 16, result
        results.append(result)
    return results


def main():
    from smoke_v7_resource_boundaries import Core, seeded_progress

    parser = argparse.ArgumentParser()
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--seconds', type=int, default=1800)
    args = parser.parse_args()
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise RuntimeError('Sustained acceptance requires an isolated hosted runner')
    if not 60 <= args.seconds <= 3600:
        raise ValueError('Soak duration must be between 60 and 3600 seconds')
    root = test_tmp()/('native-download-soak-' + uuid.uuid4().hex)
    payload = random.Random(20261113).randbytes(16*1024*1024)
    digest = hashlib.sha256(payload).hexdigest()
    stop = threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_HEAD(self):
            self.reply(False)

        def do_GET(self):
            self.reply(True)

        def reply(self, body):
            first, last = 0, len(payload)-1
            value = self.headers.get('Range')
            if value:
                start, end = value.removeprefix('bytes=').split('-')
                first, last = int(start), int(end) if end else last
            self.connection.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 65536)
            self.send_response(206 if value else 200)
            self.send_header('Content-Length', str(last-first+1))
            self.send_header('Accept-Ranges', 'bytes')
            self.send_header('ETag', '"native-soak"')
            if value:
                self.send_header('Content-Range', f'bytes {first}-{last}/{len(payload)}')
            self.end_headers()
            if not body:
                return
            began = time.monotonic()
            try:
                for offset in range(first, last+1, 65536):
                    if stop.is_set():
                        return
                    self.wfile.write(payload[offset:min(offset+65536, last+1)])
                    # Keep transfers active across many event/IPC cycles.
                    delay = (offset-first+65536)/(512*1024) - (time.monotonic()-began)
                    if delay > 0:
                        stop.wait(delay)
            except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    report = {'schema': 2, 'passed': False, 'requested_seconds': args.seconds,
              'engine_sha256': hashlib.sha256(args.engine.read_bytes()).hexdigest(),
              'scope': '1000 persisted paused tasks plus throttled real loopback range downloads; wall-clock duration',
              'samples': [], 'downloads': [], 'restarts': []}
    args.report.parent.mkdir(parents=True, exist_ok=True)

    def save():
        args.report.write_text(json.dumps(report, indent=2), encoding='utf-8')

    def sample(core, began, active, checkpoint=None):
        return {'elapsed_seconds': time.monotonic()-began, 'pid': core.process.pid,
                'state': 'active' if active is not None else 'idle', 'checkpoint': checkpoint,
                'rss_mib': core.ps.memory_info().rss/2**20,
                'handles': core.ps.num_handles(), 'cpu_seconds': sum(core.ps.cpu_times()[:2])}

    try:
        report['seeded_progress'] = seeded_progress(args.engine, root)
        save()
        with Core(args.engine, root) as core:
            began = time.monotonic()
            baseline = sample(core, began, None, 'startup')
            report['samples'].append(baseline)
            latencies = []
            active = None
            index = 0
            restarted = False
            while time.monotonic()-began < args.seconds or active is not None:
                elapsed = time.monotonic()-began
                if active is None and elapsed < args.seconds:
                    response = core.command({'kind': 'create_task', 'spec': {
                        'url': f'http://127.0.0.1:{server.server_port}/payload-{index}.bin',
                        'filename': f'payload-{index}.bin', 'resource_kind': 'file', 'concurrency': 4,
                        'proxy': 'hls-downloader://direct-proxy'}})
                    active = next(e['event']['snapshot']['task_id'] for e in response['events']
                                  if e['event']['kind'] == 'task_created')
                    index += 1
                tick = time.perf_counter()
                snapshot = core.snapshot()
                latencies.append((time.perf_counter()-tick)*1000)
                assert len(snapshot['tasks']) == 1000 + (active is not None), len(snapshot['tasks'])
                checkpoint = None
                if active:
                    task = next(t for t in snapshot['tasks'] if t['task_id'] == active)
                    assert task['status'] not in ('failed', 'canceled'), task
                    if task['status'] == 'completed':
                        assert task['active_workers'] == 0, task
                        path = Path(task['output_path'])
                        assert path.stat().st_size == len(payload)
                        with path.open('rb') as stream:
                            assert hashlib.file_digest(stream, 'sha256').hexdigest() == digest
                        core.command({'kind': 'task_action', 'task_id': active, 'action': 'delete_files'})
                        assert len(core.snapshot()['tasks']) == 1000
                        report['downloads'].append({'task_id': active, 'bytes': len(payload),
                                                    'sha256': digest, 'elapsed_seconds': elapsed})
                        active = None
                        checkpoint = 'download_deleted'
                # Compare resource ownership at business-defined idle boundaries,
                # never an idle baseline against workers with open sockets/files.
                report['samples'].append(sample(core, began, active, checkpoint))
                if not restarted and active is None and elapsed >= args.seconds/2:
                    previous = core.process.pid
                    core.__exit__()
                    core.__enter__()
                    assert len(core.snapshot()['tasks']) == 1000
                    report['restarts'].append({'previous_pid': previous, 'new_pid': core.process.pid,
                                               'elapsed_seconds': elapsed, 'tasks_preserved': 1000})
                    restarted = True
                    report['samples'].append(sample(core, began, None, 'restart'))
                if len(report['samples']) % 15 == 0:
                    save()
                    print(json.dumps({'elapsed_seconds': round(elapsed, 1),
                                      'downloads_completed': len(report['downloads']),
                                      'rss_mib': report['samples'][-1]['rss_mib']}), flush=True)
                time.sleep(1)
            report['actual_seconds'] = time.monotonic()-began
            report['snapshot_p95_ms'] = percentile(latencies, .95)
            report['snapshot_max_ms'] = max(latencies)
            report['thresholds'] = {'snapshot_p95_ms': 75, 'working_set_growth_mib': 256,
                                    'handle_growth_per_process': 16, 'download_errors': 0}
            report['samples'].append(sample(core, began, None, 'final'))
            report['resource_comparison'] = 'peak RSS across all samples; peak handle growth across comparable idle checkpoints per PID; all active samples retained'
            report['resource_growth'] = resource_growth(report['samples'])
            assert report['actual_seconds'] >= args.seconds
            assert report['downloads'] and report['restarts']
            assert report['snapshot_p95_ms'] <= 75, report['snapshot_p95_ms']
            assert core.process.poll() is None
            report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc()
    finally:
        stop.set()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        save()
    print(json.dumps({k: v for k, v in report.items() if k not in ('samples', 'downloads')}), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
