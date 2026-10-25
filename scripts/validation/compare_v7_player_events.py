"""Sandbox-only comparison of real player-status IPC with small/large task stores."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import statistics
import time
import uuid

from smoke_v7_resource_boundaries import Core
from soak_v7_runtime import percentile


def sample(engine, root, task_count):
    with Core(engine, root) as core:
        core.command({'kind': 'create_task', 'spec': {
            'url': 'http://127.0.0.1:9/event-fixture', 'filename': 'fixture.bin',
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
                    'speed_bytes_per_sec': 0, 'output_path': '',
                    'available_actions': ['resume', 'delete'],
                    'log_tail': ['fixture history ' + 'x'*100]*12}
            connection.execute('INSERT INTO tasks VALUES(?,?,?,?,?)',
                               (tid, json.dumps(task), index, index, 0))
            connection.execute('INSERT INTO task_specs VALUES(?,?)',
                               (tid, json.dumps({'url': 'http://127.0.0.1:9/fixture',
                                                'resource_kind': 'file'})))
    with Core(engine, root) as core:
        before_tasks = core.snapshot()['tasks']
        assert len(before_tasks) == task_count

        def status():
            response = core.command({'kind': 'player_control', 'action': 'status'})
            assert len(response['events']) == 1, response
            envelope = response['events'][0]
            event = envelope['event']
            assert event['kind'] == 'player_session' and not event['active'], event
            return envelope['sequence']

        for _ in range(4100):
            sequence = status()
        rss_before = core.ps.memory_info().rss
        cpu_before = sum(core.ps.cpu_times()[:2])
        started = time.perf_counter()
        latencies = []
        for _ in range(1000):
            tick = time.perf_counter()
            current = status()
            latencies.append((time.perf_counter()-tick)*1000)
            assert current == sequence + 1
            sequence = current
        elapsed = time.perf_counter()-started
        cpu = sum(core.ps.cpu_times()[:2])-cpu_before
        rss_after = core.ps.memory_info().rss
        assert core.snapshot()['tasks'] == before_tasks
        with sqlite3.connect(db) as connection:
            assert connection.execute('SELECT sequence FROM event_checkpoints').fetchone()[0] == sequence
        assert not core.ps.children(), 'Status polling unexpectedly started a player child'
        return {'tasks': task_count, 'warmup': 4100, 'commits': 1000,
                'p50_ms': round(percentile(latencies, .5), 3),
                'p95_ms': round(percentile(latencies, .95), 3),
                'max_ms': round(max(latencies), 3), 'elapsed_seconds': round(elapsed, 3),
                'cpu_seconds': round(cpu, 6), 'rss_start_mib': round(rss_before/2**20, 3),
                'rss_end_mib': round(rss_after/2**20, 3), 'last_sequence': sequence}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    engines = {'baseline-a': args.baseline, 'baseline-b': args.baseline,
               'candidate': args.candidate}
    result = {'passed': False, 'rounds': [], 'medians': {},
              'scope': 'Player status without active decoding; real IPC and SQLite checkpoint commits; identical baseline-a/b control',
              'sha256': {name: hashlib.sha256(path.read_bytes()).hexdigest()
                         for name, path in engines.items()}}
    root = Path(r'C:\QAWork')/('player-events-'+uuid.uuid4().hex)
    names = list(engines)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    try:
        for task_count in (10, 1000):
            for number in range(3):
                for name in names[number:] + names[:number]:
                    value = sample(engines[name], root/f'{task_count}-{number}-{name}', task_count)
                    result['rounds'].append({'variant': name, 'round': number+1, **value})
                    args.report.write_text(json.dumps(result, indent=2), encoding='utf-8')
            result['medians'][str(task_count)] = {
                name: {key: statistics.median(row[key] for row in result['rounds']
                        if row['variant'] == name and row['tasks'] == task_count)
                       for key in ('p50_ms', 'p95_ms', 'cpu_seconds', 'rss_end_mib')}
                for name in names}
        result['passed'] = True
    finally:
        args.report.write_text(json.dumps(result, indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
