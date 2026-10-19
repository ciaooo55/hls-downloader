import argparse
import hashlib
import json
import os
from pathlib import Path
from smoke_v7_resource_boundaries import seeded_progress

parser = argparse.ArgumentParser()
parser.add_argument('--report', type=Path, required=True)
args = parser.parse_args()
assert os.environ.get('HLS_VM_ACCEPTANCE') == '1'
engines = {'baseline': Path(r'C:\HLSBaseline\app\resources\HLSDownloaderEngine.exe'),
           'candidate': Path(r'C:\HLSRuntime\app\resources\HLSDownloaderEngine.exe')}
result = {'passed': False, 'baseline': 'previous local backend candidate', 'rounds': [],
          'sha256': {k: hashlib.sha256(v.read_bytes()).hexdigest() for k, v in engines.items()}}
try:
    for count in (10, 1000):
        for number in range(3):
            order = ('baseline', 'candidate') if number % 2 == 0 else ('candidate', 'baseline')
            for variant in order:
                measurement = seeded_progress(engines[variant], Path(r'C:\QAWork')/f'perf-{count}-{number}-{variant}', count)
                result['rounds'].append({'variant': variant, 'round': number+1, **measurement})
                args.report.write_text(json.dumps(result, indent=2), encoding='utf8')
    result['passed'] = True
finally:
    args.report.write_text(json.dumps(result, indent=2), encoding='utf8')
