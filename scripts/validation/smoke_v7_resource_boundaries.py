"""Sandbox-only resource/latency comparison and a suspended real player regression."""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import time
import uuid

import psutil

from msi_checkpoint_fixture import connect, send
from soak_v7_runtime import percentile


def wait(predicate, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.025)
    raise TimeoutError('resource fixture condition not reached')


class Core:
    def __init__(self, engine, root):
        self.engine, self.root = engine, root

    def __enter__(self):
        self.root.mkdir(parents=True, exist_ok=True)
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            self.port = probe.getsockname()[1]
        env = {**os.environ, 'HLS_V7_DATA_DIR': str(self.root/'data'),
               'HLS_V7_DOWNLOAD_DIR': str(self.root/'downloads'), 'HLS_V7_CORE_TCP': '1',
               'HLS_V7_CORE_BIND': f'127.0.0.1:{self.port}',
               'HLS_V7_PIPE': rf'\\.\pipe\HLSResource-{uuid.uuid4().hex}',
               'HLS_V6_SKIP_MIGRATE': '1'}
        self.log = (self.root/'core.log').open('ab')
        self.process = subprocess.Popen([str(self.engine)], env=env,
            stdout=self.log, stderr=self.log, creationflags=subprocess.CREATE_NO_WINDOW)
        self.ps = psutil.Process(self.process.pid)
        try:
            self.ipc = connect(self.port)
            self.ipc.settimeout(25)
            self.call({'type': 'store_settings', 'request_id': 1, 'values': {
                'download_progress_window_enabled': False, 'download_complete_popup_enabled': False,
                'av_scan_enabled': False, 'auto_retry_failed_max': 0}})
            return self
        except BaseException:
            self.__exit__()
            raise

    def call(self, value):
        response = send(self.ipc, value)
        assert response.get('type') != 'error', response
        return response

    def command(self, value):
        return self.call({'type': 'command', 'request_id': 2, 'command': value})

    def snapshot(self):
        return self.call({'type': 'snapshot', 'request_id': 3})

    def __exit__(self, *_):
        if getattr(self, 'ipc', None):
            self.ipc.close()
        children = self.ps.children(recursive=True) if self.process.poll() is None else []
        for child in children:
            try:
                child.kill()
            except psutil.NoSuchProcess:
                pass
        self.process.terminate()
        self.process.wait(timeout=15)
        self.log.close()


def seeded_progress(engine, root, task_count=1000, *, repeated=False):
    assert 1 <= task_count <= 1000
    with Core(engine, root) as core:
        core.command({'kind': 'create_task', 'spec': {
            'url': 'http://127.0.0.1:9/resource-fixture', 'filename': 'fixture.bin',
            'resource_kind': 'file', 'proxy': 'hls-downloader://direct-proxy'}})
        template = core.snapshot()['tasks'][0]
    db = root/'data/data.db'
    with sqlite3.connect(db) as connection:
        connection.execute('DELETE FROM tasks')
        connection.execute('DELETE FROM task_specs')
        for index in range(task_count):
            tid = f'fixture-{index}'
            task = {**template, 'task_id': tid, 'status': 'paused', 'stage': 'transfer',
                    'downloaded_bytes': 0, 'total_bytes': 1_000_000_000,
                    'speed_bytes_per_sec': 0, 'output_path': '', 'available_actions': ['resume','delete'],
                    'log_tail': ['fixture history ' + 'x'*100]*12}
            connection.execute('INSERT INTO tasks VALUES(?,?,?,?,?)',
                               (tid, json.dumps(task), index, index, 0))
            connection.execute('INSERT INTO task_specs VALUES(?,?)',
                               (tid, json.dumps({'url': 'http://127.0.0.1:9/fixture', 'resource_kind': 'file'})))
        if repeated:
            # 同一任务已有满额日志，后续进度只改变速度、不改变字节数和阶段。
            connection.execute('DELETE FROM logs')
            for index in range(500):
                connection.execute(
                    'INSERT INTO logs(task_id,level,code,message,created_at_ms) VALUES(?,?,?,?,?)',
                    ('fixture-0', 'progress', 'transfer',
                     f'paused transfer {index}/1000000000', index))
            log_rows_before = connection.execute('SELECT * FROM logs ORDER BY id').fetchall()
    with Core(engine, root) as core:
        assert len(core.snapshot()['tasks']) == task_count
        # Fill the runtime event ring; then measure the same 1000 progress commits.
        for index in range(4100):
            core.command({'kind': 'get_task', 'task_id': f'fixture-{index%task_count}'})
        before = core.ps.memory_info().rss
        cpu_before = sum(core.ps.cpu_times()[:2])
        started = time.perf_counter()
        latencies = []
        for index in range(1000):
            tick = time.perf_counter()
            core.command({'kind': 'update_progress',
                          'task_id': 'fixture-0' if repeated else f'fixture-{index%task_count}',
                          'downloaded_bytes': 499 if repeated else index+1,
                          'total_bytes': 1_000_000_000,
                          'speed_bytes_per_sec': index % 2 if repeated else 0,
                          'stage': 'transfer', 'status': 'paused'})
            latencies.append((time.perf_counter()-tick)*1000)
        elapsed = time.perf_counter()-started
        cpu = sum(core.ps.cpu_times()[:2])-cpu_before
        tasks = core.snapshot()['tasks']
        assert len(tasks) == task_count and all(t['status'] == 'paused' for t in tasks)
        expected_bytes = ({f'fixture-{i}': 499 if i == 0 else 0 for i in range(task_count)}
                          if repeated else {f'fixture-{i%task_count}': i+1 for i in range(1000)})
        assert all(t['downloaded_bytes'] == expected_bytes[t['task_id']] for t in tasks)
        if repeated:
            with sqlite3.connect(db) as connection:
                assert connection.execute('SELECT * FROM logs ORDER BY id').fetchall() == log_rows_before
        return {'tasks': task_count, 'event_ring_warmup': 4100, 'commits': 1000,
                'progress_pattern': 'repeated-full-log' if repeated else 'changing',
                'p50_ms': round(percentile(latencies, .5), 3),
                'p95_ms': round(percentile(latencies, .95), 3),
                'max_ms': round(max(latencies), 3), 'elapsed_seconds': round(elapsed, 3),
                'cpu_seconds': round(cpu, 3), 'cpu_one_core_percent': round(cpu/elapsed*100, 3),
                'rss_start_mib': round(before/2**20, 3),
                'rss_end_mib': round(core.ps.memory_info().rss/2**20, 3)}


def suspended_player(engine, root, media):
    output = root/'downloads/player.mp4'
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(media, output)
    with Core(engine, root) as core:
        core.command({'kind': 'create_task', 'spec': {
            'url': 'http://127.0.0.1:9/player-fixture', 'filename': 'player.mp4',
            'resource_kind': 'file'}})
        template = core.snapshot()['tasks'][0]
    with sqlite3.connect(root/'data/data.db') as connection:
        template.update(status='completed', stage='completed', output_path=str(output),
                        output_missing=False, playback_ready=True)
        connection.execute('UPDATE tasks SET snapshot_json=? WHERE task_id=?',
                           (json.dumps(template), template['task_id']))
    with Core(engine, root) as core:
        tid = template['task_id']
        def status():
            value = core.command({'kind': 'player_control', 'action': 'status'})
            return next(e['event'] for e in value['events'] if e['event']['kind'] == 'player_session')
        core.command({'kind': 'play_task', 'task_id': tid})
        wait(lambda: status()['position_available'])
        child = next(p for p in core.ps.children() if '--player-process' in p.cmdline())
        child.suspend()
        try:
            with ThreadPoolExecutor(max_workers=1) as executor:
                started = time.perf_counter()
                blocked = executor.submit(status)
                # A second IPC connection must remain useful while player I/O is stuck.
                with connect(core.port) as other:
                    tick = time.perf_counter()
                    assert send(other, {'type': 'snapshot', 'request_id': 88})['tasks']
                    snapshot_ms = (time.perf_counter()-tick)*1000
                value = blocked.result(timeout=15)
                timeout_seconds = time.perf_counter()-started
            assert 8 < timeout_seconds < 14, timeout_seconds
            assert not value['active'], value
            wait(lambda: not child.is_running())
            core.command({'kind': 'play_task', 'task_id': tid})
            wait(lambda: status()['position_available'])
            replacement = next(p for p in core.ps.children() if '--player-process' in p.cmdline())
            assert replacement.pid != child.pid
            core.command({'kind': 'player_control', 'action': 'stop'})
            assert not status()['active']
            return {'timeout_seconds': round(timeout_seconds, 3),
                    'concurrent_snapshot_ms': round(snapshot_ms, 3),
                    'old_pid': child.pid, 'replacement_pid': replacement.pid, 'passed': True}
        finally:
            if child.is_running():
                child.resume()
                child.kill()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--media', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--player-only', action='store_true')
    args = parser.parse_args()
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    root = Path(r'C:\QAWork')/('resources-'+uuid.uuid4().hex)
    result = {'passed': False, 'scope': 'provided baseline and candidate binaries; same VM/fixture; identities recorded below',
              'sha256': {name: hashlib.sha256(path.read_bytes()).hexdigest()
                         for name, path in [('baseline',args.baseline),('candidate',args.candidate)]}}
    try:
        if not args.player_only:
            for name, engine in [('baseline',args.baseline),('candidate',args.candidate)]:
                result[name] = seeded_progress(engine, root/name)
                args.report.write_text(json.dumps(result, indent=2), encoding='utf-8')
        result['suspended_player'] = suspended_player(args.candidate, root/'player', args.media)
        result['passed'] = True
    finally:
        args.report.write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
