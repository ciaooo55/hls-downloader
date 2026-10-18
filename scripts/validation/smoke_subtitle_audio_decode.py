"""沙盒内复验伪装后缀 HLS、无后缀清单和普通音频的真实预取解码。"""
import argparse
import functools
import http.server
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    report = {'passed': False}
    server = None
    with tempfile.TemporaryDirectory(prefix='hls-subtitle-decode-') as temporary:
        root = Path(temporary)

        def ff(*values):
            subprocess.run([str(args.ffmpeg), '-nostdin', '-y', '-v', 'error', *map(str, values)],
                           cwd=root, check=True, capture_output=True, timeout=30)

        try:
            ff('-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=16000', '-t', '6',
               '-c:a', 'aac', 'sample.m4a')
            ff('-i', 'sample.m4a', '-c', 'copy', '-hls_time', '1', '-hls_list_size', '0',
               '-hls_segment_filename', 'part%02d.jpeg', 'index.m3u8')
            shutil.copyfile(root / 'index.m3u8', root / 'opaque')
            (root / 'local.m3u8').write_text('#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\n'
                                           + (root / 'sample.m4a').as_uri() + '\n#EXT-X-ENDLIST\n')

            class Handler(http.server.SimpleHTTPRequestHandler):
                def log_message(self, *_args):
                    pass

            server = http.server.ThreadingHTTPServer(('127.0.0.1', 0),
                functools.partial(Handler, directory=str(root)))
            threading.Thread(target=server.serve_forever, daemon=True).start()
            env = {**os.environ, 'HLS_FFMPEG': str(args.ffmpeg),
                   'HLS_SUBTITLE_FIXTURE_ORIGIN': f'http://127.0.0.1:{server.server_port}'}
            result = subprocess.run([str(args.tests_binary),
                'subtitle_audio_source::tests::decode_disguised_hls_audio', '--exact', '--ignored'],
                env=env, capture_output=True, timeout=60)
            report.update(passed=result.returncode == 0 and b'1 passed' in result.stdout, exit_code=result.returncode,
                          stdout=result.stdout.decode(errors='replace'),
                          stderr=result.stderr.decode(errors='replace'))
        finally:
            if server:
                server.shutdown()
                server.server_close()
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--ffmpeg', type=Path, required=True)
    parser.add_argument('--tests-binary', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
