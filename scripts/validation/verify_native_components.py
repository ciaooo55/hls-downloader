"""Exercise dependency-gated Rust tests on disposable hosted Windows."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
from validation_workspace import test_tmp


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    report_path = args.report.resolve()
    if os.name != 'nt' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted':
        parser.error('requires disposable GitHub-hosted Windows; never run on a personal desktop')
    if not report_path.is_relative_to(root):
        parser.error('report must remain in the project workspace')
    resources = args.runtime.resolve() / 'app/resources'
    tools = {'ffmpeg': resources / 'ffmpeg.exe',
             'curl': resources / 'tools/curl-impersonate/curl-impersonate.exe'}
    for name, path in tools.items():
        if not path.is_file():
            parser.error(f'missing packaged {name}: {path}')
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'source_commit': subprocess.check_output(
        ['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(), 'checks': [],
        'tools': {name: {'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
                  for name, path in tools.items()},
        'scope': 'Real packaged FFmpeg/curl and Windows APIs; no physical-device, cloud-quality or interactive-UAC assertion'}

    with tempfile.TemporaryDirectory(prefix='native-components-', dir=test_tmp()) as temporary:
        environment = dict(os.environ, TEMP=temporary, TMP=temporary, HLS_VM_ACCEPTANCE='1',
                           HLS_FFMPEG=str(tools['ffmpeg']), HLS_V7_CURL_IMPERSONATE=str(tools['curl']))

        def execute(command, name, timeout=90):
            print(f'Running {name}', flush=True)
            started = time.monotonic()
            process = subprocess.Popen(command, cwd=root, env=environment,
                                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
            try:
                output, _ = process.communicate(timeout=timeout)
            except subprocess.TimeoutExpired:
                subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'],
                               capture_output=True, timeout=20)
                process.communicate()
                raise RuntimeError(f'{name} timed out; owned process tree terminated')
            text = output.decode('utf-8', errors='replace')
            log = report_path.parent / f'component-{name}.log'
            log.write_text(text, encoding='utf-8')
            if process.returncode:
                raise RuntimeError(f'{name} exited {process.returncode}: {text[-3000:]}')
            return text, round(time.monotonic() - started, 3)

        def build(package):
            output, _ = execute(['cargo', 'test', '--locked', '-p', package, '--lib',
                                 '--no-run', '--message-format=json'], f'build-{package}', 900)
            artifacts = [json.loads(line) for line in output.splitlines() if line.startswith('{')]
            binaries = [row['executable'] for row in artifacts
                        if row.get('reason') == 'compiler-artifact' and row.get('executable')
                        and row.get('target', {}).get('kind') == ['lib']]
            if len(binaries) != 1:
                raise RuntimeError(f'expected one {package} test binary, received {binaries}')
            return Path(binaries[0])

        try:
            for package in ('hls-native-shell', 'hls-client'):
                binary = build(package)
                inventory, _ = execute([str(binary), '--list', '--ignored'], f'list-{package}')
                tests = [line.removesuffix(': test') for line in inventory.splitlines()
                         if line.endswith(': test')]
                if not tests:
                    raise RuntimeError(f'no dependency-gated tests found in {package}')
                report.setdefault('test_binaries', {})[package] = hashlib.sha256(binary.read_bytes()).hexdigest()
                for index, name in enumerate(tests):
                    check = {'name': name, 'passed': False}
                    report['checks'].append(check)
                    try:
                        if name == 'av_scan::tests::scanner_timeout_fixture':
                            # This is a child-process fixture, not an untested scanner feature.
                            parent = 'av_scan::tests::timeout_terminates_scanner_process'
                            output, elapsed = execute([str(binary), parent, '--exact'], f'{package}-{index}')
                            check['classification'] = 'helper exercised by real scanner-timeout regression'
                        elif name == 'subtitle_audio_source::tests::decode_disguised_hls_audio':
                            # The existing fixture owns its HTTP server and real media generation.
                            started = time.monotonic()
                            fixture_report = report_path.parent / 'component-subtitle-audio.json'
                            command = [os.sys.executable, str(root / 'scripts/validation/smoke_subtitle_audio_decode.py'),
                                       '--ffmpeg', str(tools['ffmpeg']), '--tests-binary', str(binary),
                                       '--report', str(fixture_report)]
                            execute(command, f'{package}-{index}', 120)
                            fixture = json.loads(fixture_report.read_text(encoding='utf-8'))
                            if not fixture['passed']:
                                raise RuntimeError('subtitle audio fixture failed')
                            output = fixture['stdout']
                            elapsed = round(time.monotonic() - started, 3)
                        else:
                            output, elapsed = execute([str(binary), name, '--exact', '--ignored'],
                                                      f'{package}-{index}')
                        if not re.search(r'test result: ok\. 1 passed; 0 failed; 0 ignored;', output):
                            raise RuntimeError('test did not execute and pass exactly one case')
                        check.update(passed=True, seconds=elapsed)
                    except Exception as error:
                        check['error'] = str(error)
            report['passed'] = bool(report['checks']) and all(check['passed'] for check in report['checks'])
            report['functional_tests'] = sum('classification' not in check for check in report['checks'])
            report['helper_regressions'] = sum('classification' in check for check in report['checks'])
        except Exception as error:
            report['error'] = str(error)
        finally:
            report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({'passed': report['passed'], 'checks': len(report['checks'])}))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
