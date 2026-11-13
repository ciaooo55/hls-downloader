"""Bounded real Internet HTTPS download of the frozen repository README."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import traceback
from urllib.request import Request, urlopen
import uuid

from smoke_v7_resource_boundaries import Core, wait
from validation_workspace import test_tmp


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--source', required=True)
    parser.add_argument('--repository', required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    if os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted':
        raise RuntimeError('Public HTTPS acceptance requires an isolated hosted runner')
    if not re.fullmatch(r'[0-9a-f]{40}', args.source) or not re.fullmatch(r'[\w.-]+/[\w.-]+', args.repository):
        raise ValueError('Invalid frozen public source')
    url = f'https://raw.githubusercontent.com/{args.repository}/{args.source}/README.md'
    report = {'passed': False, 'url': url, 'source_commit': args.source,
              'engine_sha256': hashlib.sha256(args.engine.read_bytes()).hexdigest(),
              'scope': 'One actual public HTTPS origin and frozen file; no whole-Internet compatibility assertion'}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    try:
        with urlopen(Request(url, headers={'User-Agent': 'HLS-release-acceptance'}), timeout=30) as response:
            expected = response.read(2*1024*1024+1)
        assert 0 < len(expected) <= 2*1024*1024
        digest = hashlib.sha256(expected).hexdigest()
        root = test_tmp()/('public-https-' + uuid.uuid4().hex)
        with Core(args.engine, root) as core:
            result = core.command({'kind': 'create_task', 'spec': {
                'url': url, 'filename': 'frozen-README.md', 'resource_kind': 'file', 'concurrency': 4}})
            task_id = next(e['event']['snapshot']['task_id'] for e in result['events']
                           if e['event']['kind'] == 'task_created')
            def finished():
                task = next(t for t in core.snapshot()['tasks'] if t['task_id'] == task_id)
                return task if task['status'] in ('completed', 'failed') else None
            task = wait(finished, 60)
            assert task['status'] == 'completed', task
            actual = Path(task['output_path']).read_bytes()
            assert len(actual) == len(expected) and hashlib.sha256(actual).hexdigest() == digest
            report.update(passed=True, bytes=len(actual), sha256=digest)
            core.command({'kind': 'task_action', 'task_id': task_id, 'action': 'delete_files'})
    except Exception:
        report['error'] = traceback.format_exc()
    finally:
        args.report.write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
