"""Repeat the packaged product's automatic acceptance matrix in Windows Sandbox.

The round number changes player DOM/scale boundaries, not the set of features.
Cloud providers and physical casting receivers are deliberately outside this matrix.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

import psutil


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Windows Sandbox only'
    scripts = Path(__file__).parent
    args.report.mkdir(parents=True, exist_ok=True)
    resources = args.runtime / 'app/resources'
    os.environ['PYTHONPATH'] = str(args.extension.parent / 'python-deps')
    engine = resources / 'HLSDownloaderEngine.exe'
    work = Path(r'C:\QAWork') / f'adversarial-round-{args.round}'
    work.mkdir(parents=True, exist_ok=True)
    result = {'round': args.round, 'passed': False, 'checks': [],
              'engine_sha256': hashlib.sha256(engine.read_bytes()).hexdigest(),
              'background_sha256': hashlib.sha256((args.extension / 'background.js').read_bytes()).hexdigest(),
              'limitations': ['fixture subtitle provider; no cloud quality claim',
                              'no physical casting/TVBox receiver',
                              'UI test API does not prove native mouse hit testing',
                              'Chromium browser takeover is not covered by Firefox acceptance']}

    def save():
        (args.report / 'summary.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')

    def stop_runtime():
        retained = []
        processes = []
        for process in psutil.process_iter(['exe', 'pid', 'name']):
            if process.info['exe'] and Path(process.info['exe']).is_relative_to(args.runtime):
                retained.append({'pid': process.pid, 'name': process.info['name']})
                try:
                    process.terminate()
                    processes.append(process)
                except psutil.NoSuchProcess:
                    pass
        _, alive = psutil.wait_procs(processes, timeout=5)
        for process in alive:
            try: process.kill()
            except psutil.NoSuchProcess: pass
        return retained

    def case(name, script, arguments, timeout=300):
        report = args.report / (name + '.json')
        command = [sys.executable, str(scripts / script), *map(str, arguments), '--report', str(report)]
        started = time.monotonic()
        record = {'name': name, 'passed': False, 'command': command}
        result['checks'].append(record)
        save()
        with (args.report / (name + '.log')).open('wb') as log:
            process = subprocess.Popen(command, cwd=work, stdout=log, stderr=log,
                                       creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                record['exit_code'] = process.wait(timeout=timeout)
                if report.exists():
                    details = json.loads(report.read_text(encoding='utf-8-sig'))
                    # Media acceptance returns a scenario list and raises on failed assertions.
                    record['passed'] = record['exit_code'] == 0 and (
                        bool(details) if isinstance(details, list) else details.get('passed') is True)
                    if isinstance(details, list):
                        record['scenario_count'] = len(details)
                    else:
                        record['check_count'] = len(details.get('checks', []))
                else:
                    record['error'] = 'No result report was produced'
            except Exception as error:
                record['error'] = str(error)
            finally:
                # Only descendants of this test process; never terminate a user's app.
                if process.poll() is None:
                    try:
                        parent = psutil.Process(process.pid)
                        for child in parent.children(recursive=True):
                            try: child.kill()
                            except psutil.NoSuchProcess: pass
                        parent.kill()
                        process.wait(timeout=10)
                    except psutil.NoSuchProcess:
                        pass
                record['elapsed_seconds'] = round(time.monotonic() - started, 3)
                # Suites use different stores/transports. A resident process from
                # one suite must not own the next suite's desktop singleton.
                record['retained_runtime_processes'] = stop_runtime()
                save()

    env = os.environ
    env['HLS_V7_DATA_DIR'] = str(work / 'default-data')
    env['HLS_V7_DOWNLOAD_DIR'] = str(work / 'default-downloads')
    # The takeover suite exercises the default pipe and lets the real Host start Core.
    env.pop('HLS_V7_PIPE', None)
    env.pop('HLS_V7_CORE_TCP', None)
    env.pop('HLS_V7_CORE_BIND', None)
    media_args = ['--browser', 'firefox', '--extension', args.extension,
                  '--browser-binary', args.firefox, '--driver', args.driver,
                  '--ffmpeg', resources / 'ffmpeg.exe']
    case('media', 'smoke_extension_media.py', media_args, 480)
    case('takeover', 'smoke_firefox_response_takeover.py', [
        '--runtime', args.runtime, '--extension', args.extension,
        '--binary', args.firefox, '--driver', args.driver, '--public'], 480)
    dom = ['document', 'shadow', 'dynamic-shadow', 'nested-shadow', 'document'][args.round - 1]
    subtitle_args = ['--browser', 'firefox', '--extension', args.extension,
                     '--host', resources / 'HLSDownloaderNativeHost.exe',
                     '--browser-binary', args.firefox, '--firefox-driver', args.driver,
                     '--player-dom', dom]
    case('subtitle-element', 'smoke_extension_subtitles.py', [*subtitle_args, '--capture', 'element'], 240)
    case('subtitle-prefetch', 'smoke_extension_subtitles.py', [*subtitle_args, '--capture', 'prefetch'], 240)
    case('http-boundaries', 'smoke_v7_http_boundaries.py', ['--engine', engine], 300)
    case('http-stall', 'smoke_v7_http_stall.py', ['--engine', engine, '--root', work / 'http-stall'], 180)
    case('http-throttle', 'smoke_v7_http_stall.py', [
        '--engine', engine, '--root', work / 'http-throttle', '--throttle-only'], 180)
    case('closed-diagnostics', 'smoke_v7_closed_diagnostics.py', ['--engine', engine], 120)
    case('hls-segment-suffix', 'smoke_v7_hls_segment_suffix.py', ['--engine', engine, '--tools', resources], 300)
    case('file-extensions', 'smoke_v7_file_extensions.py', ['--runtime', args.runtime], 240)
    case('download-playback-cleanup', 'smoke_v7_download_playback.py', ['--runtime', args.runtime], 600)

    # Generate a small deterministic multi-track fixture without downloading media.
    media = work / 'multitrack.mkv'
    subtitles = work / 'captions.srt'
    subtitles.write_text('1\n00:00:00,000 --> 00:01:00,000\nPlayback acceptance\n', encoding='utf-8')
    fixture = subprocess.run([str(resources / 'ffmpeg.exe'), '-nostdin', '-v', 'error', '-y',
        '-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=25',
        '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=48000',
        '-f', 'lavfi', '-i', 'sine=frequency=880:sample_rate=48000', '-i', str(subtitles),
        '-map', '0:v', '-map', '1:a', '-map', '2:a', '-map', '3:s', '-map', '3:s', '-t', '60',
        '-c:v', 'libx264', '-preset', 'ultrafast', '-c:a', 'aac', '-c:s', 'srt', str(media)],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120, creationflags=subprocess.CREATE_NO_WINDOW)
    if fixture.returncode:
        result['fixture_error'] = fixture.stderr.decode(errors='replace')
    else:
        case('player-overlay', 'smoke_v7_player_overlay.py', ['--runtime', args.runtime, '--media', media,
             '--ui-scale', '1.5' if args.round % 2 == 0 else '1.0'], 240)
        case('player-fault-resource', 'smoke_v7_resource_boundaries.py', [
            '--baseline', engine, '--candidate', engine, '--media', media], 300)
    if args.round == 5:
        case('workbench', 'smoke_v7_workbench_interactions.py', ['--runtime', args.runtime], 1200)
    # LAN sharing intentionally opens an external listener and may prompt Windows.
    # Run it after UI acceptance; each round uses a fresh Sandbox.
    case('protocols', 'smoke_v7_protocol_features.py', ['--engine', engine, '--no-firewall-rule'], 600)
    expected_checks = 15 if args.round == 5 else 14
    result['passed'] = not result.get('fixture_error') and len(result['checks']) == expected_checks and all(c['passed'] for c in result['checks'])
    save()
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--round', type=int, choices=range(1, 6), required=True)
    for name in ('runtime', 'extension', 'firefox', 'driver', 'report'):
        parser.add_argument('--' + name, type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
