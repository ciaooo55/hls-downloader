"""Sandbox-only Core comparison with an identical-binary control and repeated logs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import uuid

from smoke_v7_resource_boundaries import seeded_progress


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
              'scope': 'previous backend versus candidate; baseline-a/b use the same executable',
              'sha256': {k: hashlib.sha256(v.read_bytes()).hexdigest() for k, v in engines.items()}}
    root = Path(r'C:\QAWork') / ('log-comparison-' + uuid.uuid4().hex)
    names = list(engines)
    try:
        for task_count in (10, 1000):
            for repeated in (False, True):
                for number in range(3):
                    # 轮换首尾位置；同二进制对照用于观察独立进程/测量自身的波动。
                    for name in names[number:] + names[:number]:
                        value = seeded_progress(engines[name],
                            root/f'{task_count}-{repeated}-{number}-{name}',
                            task_count, repeated=repeated)
                        result['rounds'].append({'variant': name, 'round': number+1, **value})
                        args.report.write_text(json.dumps(result, indent=2), encoding='utf8')
            result['medians'][str(task_count)] = {}
            for pattern in ('changing', 'repeated-full-log'):
                result['medians'][str(task_count)][pattern] = {
                    name: {key: statistics.median(row[key] for row in result['rounds']
                            if row['variant'] == name and row['tasks'] == task_count
                            and row['progress_pattern'] == pattern)
                           for key in ('p50_ms', 'p95_ms', 'cpu_seconds', 'rss_end_mib')}
                    for name in names}
        result['passed'] = True
    finally:
        args.report.write_text(json.dumps(result, indent=2), encoding='utf8')


if __name__ == '__main__':
    main()
