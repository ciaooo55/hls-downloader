"""Headless, isolated Core HLS validation using real media and exact credentials."""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import threading
import uuid
from urllib.parse import urlsplit

from smoke_v7_resource_boundaries import Core, wait
from validation_workspace import test_tmp


def run(args):
    root = test_tmp() / 'idm-streaming-20261116' / ('real-media-' + uuid.uuid4().hex[:8])
    root.mkdir(parents=True, exist_ok=False)
    fixture = root / 'fixture'
    fixture.mkdir()
    report = {'passed': False, 'cases': [], 'engine_sha256': hashlib.sha256(args.engine.read_bytes()).hexdigest(),
              'scope': 'Isolated headless Core, loopback HTTP, generated legal media; no desktop, browser, IDM or release install'}
    signature = 'sig=a%2Fb+z'
    requests = Counter()
    attempts = Counter()
    request_lock = threading.Lock()
    ranges = []
    expected_ranges = []
    denied = []
    kinds = ('plain', 'encrypted', 'fmp4', 'range-fmp4')
    server = thread = None

    def ff(*params, cwd=None):
        result = subprocess.run([str(args.ffmpeg), '-nostdin', '-y', '-v', 'error', *map(str, params)],
                                cwd=cwd, capture_output=True, timeout=60,
                                creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        assert result.returncode == 0, result.stderr.decode(errors='replace')
        return result.stdout

    def decoded(path, stream):
        result = ff('-i', path, '-map', stream, '-f', 'framemd5', '-')
        return [[field.strip() for field in line.split(b',')] for line in result.splitlines()
                if line and not line.startswith(b'#')]

    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *params, **kw):
            super().__init__(*params, directory=str(fixture), **kw)

        def log_message(self, *_):
            pass

        def do_GET(self):
            parsed = urlsplit(self.path)
            if (parsed.query != signature or self.headers.get('Authorization') != 'Bearer hls-fixture'
                    or 'sid=exact-hls' not in self.headers.get('Cookie', '')):
                denied.append(self.path)
                self.send_error(403)
                return
            with request_lock:
                requests[parsed.path] += 1
                identity = (parsed.path, self.headers.get('Range', ''))
                attempts[identity] += 1
                first_attempt = attempts[identity] == 1
            if args.curl and first_attempt:
                # The ordinary HTTP client must fall back to the supplied curl
                # transport. No personal browser or external service is used.
                self.send_response(403)
                self.send_header('Content-Length', '0')
                self.end_headers()
                return
            path = fixture / parsed.path.lstrip('/')
            if parsed.path.startswith('/range-fmp4/') and path.suffix == '.m4s':
                value = self.headers.get('Range', '')
                match = re.fullmatch(r'bytes=(\d+)-(\d+)', value)
                if not match:
                    self.send_error(416)
                    return
                body = path.read_bytes()
                start, end = map(int, match.groups())
                if not 0 <= start <= end < len(body):
                    self.send_error(416)
                    return
                ranges.append(value)
                self.send_response(206)
                self.send_header('Content-Range', f'bytes {start}-{end}/{len(body)}')
                self.send_header('Content-Length', str(end - start + 1))
                self.end_headers()
                self.wfile.write(body[start:end + 1])
            elif path.suffix in ('.ts', '.m4s'):
                body = path.read_bytes()
                self.protocol_version = 'HTTP/1.1'
                self.send_response(200)
                self.send_header('Transfer-Encoding', 'chunked')
                self.send_header('Connection', 'close')
                self.end_headers()
                for start in range(0, len(body), 1003):
                    chunk = body[start:start + 1003]
                    self.wfile.write(f'{len(chunk):x}\r\n'.encode() + chunk + b'\r\n')
                self.wfile.write(b'0\r\n\r\n')
                self.close_connection = True
            else:
                super().do_GET()

    try:
        source = fixture / 'source.mp4'
        ff('-f', 'lavfi', '-i', 'testsrc2=size=160x90:rate=10', '-f', 'lavfi', '-i',
           'sine=frequency=440:sample_rate=48000', '-t', '3', '-c:v', 'libx264', '-pix_fmt',
           'yuv420p', '-g', '10', '-c:a', 'aac', source)
        for kind in kinds:
            folder = fixture / kind
            folder.mkdir()
            extra = []
            if kind == 'encrypted':
                key = folder / 'key.bin'
                key.write_bytes(bytes(range(16)))
                info = folder / 'key-info.txt'
                info.write_text(f'key.bin\n{key}\n', encoding='utf-8')
                extra = ['-hls_key_info_file', info]
            elif kind in ('fmp4', 'range-fmp4'):
                extra = ['-hls_segment_type', 'fmp4']
                if kind == 'range-fmp4':
                    extra += ['-hls_flags', 'single_file']
            playlist = folder / 'index.m3u8'
            ff('-i', source, '-c', 'copy', '-hls_time', '1', '-hls_playlist_type', 'vod',
               *extra, playlist, cwd=folder)
            # Independent FFmpeg reads the original encrypted/plain playlist directly.
            reference = folder / 'reference.mp4'
            ff('-protocol_whitelist', 'file,crypto', '-allowed_extensions', 'ALL',
               '-i', playlist, '-c', 'copy', '-movflags', '+faststart', reference)
            text = playlist.read_text()
            if kind == 'range-fmp4':
                # FFmpeg's single-file fixture supplies explicit offsets for every range.
                entries = re.findall(r'(?:#EXT-X-BYTERANGE:|BYTERANGE=")(\d+)@(\d+)', text)
                assert len(entries) == 4, text
                expected_ranges = [f'bytes={offset}-{int(offset) + int(length) - 1}'
                                   for length, offset in entries]
            text = re.sub(r'URI="([^"]+)"', lambda m: f'URI="{m[1]}?{signature}"', text)
            text = '\n'.join(line + '?' + signature if line and not line.startswith('#') else line
                             for line in text.splitlines()) + '\n'
            playlist.write_text(text)
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        with Core(args.engine, root / 'core', environment={'HLS_FFMPEG': str(args.ffmpeg),
                  'HLS_V7_CURL_IMPERSONATE': str(args.curl) if args.curl else '',
                  'TEMP': str(root), 'TMP': str(root)}) as core:
            core.call({'type': 'store_settings', 'request_id': 4, 'values': {
                'legal_terms_accepted': True, 'legal_terms_version': '2026-08-06-cn-1', 'keep_temp_files': True}})
            for kind in kinds:
                created = core.command({'kind': 'create_task', 'spec': {
                    'url': f'http://127.0.0.1:{server.server_port}/{kind}/index.m3u8?{signature}',
                    'filename': f'{kind}.mp4', 'resource_kind': 'hls', 'concurrency': 3,
                    'proxy': 'hls-downloader://direct-proxy',
                    'headers': {'authorization': 'Bearer hls-fixture', 'cookie': 'sid=exact-hls'}, 'start_paused': False}})
                tid = next(e['event']['snapshot']['task_id'] for e in created['events']
                           if e['event'].get('kind') == 'task_created')
                def completed():
                    task = next(t for t in core.snapshot()['tasks'] if t['task_id'] == tid)
                    assert task['status'] != 'failed', task
                    return task if task['status'] == 'completed' else None
                task = wait(completed, timeout=60)
                output = Path(task['output_path'])
                reference = fixture / kind / 'reference.mp4'
                evidence = {'kind': kind, 'output_sha256': hashlib.sha256(output.read_bytes()).hexdigest()}
                for stream, name in (('0:v:0', 'video'), ('0:a:0', 'audio')):
                    expected, actual = decoded(reference, stream), decoded(output, stream)
                    assert actual and actual == expected, f'{kind}: decoded {name} frame/sample hash mismatch'
                    evidence[name + '_frames'] = len(actual)
                ff('-xerror', '-i', output, '-map', '0:v:0', '-map', '0:a:0', '-f', 'null', '-')
                assert not list((root / 'core').rglob('*.part')), 'provisional segments remained after success'
                assert not list((root / 'core').rglob('*.response.json')), 'unused HTTP response sidecars remained'
                report['cases'].append(evidence)
        assert not denied, denied
        assert requests['/encrypted/key.bin'] > 0 and any('/fmp4/' in path and path.endswith('.mp4') for path in requests)
        assert Counter(ranges) == Counter(expected_ranges), (ranges, expected_ranges)
        if args.curl:
            assert all(count == 2 for count in attempts.values()), attempts
        report['passed'] = True
    except BaseException as error:
        report['error'] = repr(error)
        raise
    finally:
        if server:
            server.shutdown()
            server.server_close()
        if thread:
            thread.join(timeout=5)
        report['work_dir'] = str(root)
        report['requests'] = dict(requests)
        report['denied_requests'] = denied
        report['range_requests'] = ranges
        report['expected_ranges'] = expected_ranges
        report['transport'] = 'forced curl fallback after first HTTP 403' if args.curl else 'direct HTTP'
        report['attempts'] = [{'path': path, 'range': value, 'count': count}
                              for (path, value), count in sorted(attempts.items())]
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2), encoding='utf-8')
        # Keep diagnostics on failure; remove only this script's successful fixture.
        if report['passed']:
            shutil.rmtree(root)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--engine', type=lambda p: Path(p).resolve(strict=True), required=True)
    parser.add_argument('--ffmpeg', type=lambda p: Path(p).resolve(strict=True), required=True)
    parser.add_argument('--curl', type=lambda p: Path(p).resolve(strict=True))
    parser.add_argument('--report', type=Path, required=True)
    run(parser.parse_args())
