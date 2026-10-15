"""Guest-only regression for HLS segments whose URL suffix is not their format."""
from pathlib import Path
import argparse
from collections import Counter
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import shutil
import subprocess
import threading
import traceback
import uuid

from msi_checkpoint_fixture import connect, send, wait_task
from smoke_v7_transfer_performance import free_port


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Run in the acceptance VM'
    root = Path(r'C:\QAWork') / ('suffix-regression-' + uuid.uuid4().hex)
    root.mkdir(parents=True, exist_ok=True)
    fixture = root / 'fixture'
    fixture.mkdir(exist_ok=True)
    report = {'passed': False, 'checks': []}
    process = server = None
    gate = threading.Event()
    requested = threading.Event()
    counts = Counter()
    ffmpeg = str(args.tools / 'ffmpeg.exe')

    def ff(*argv):
        return subprocess.run([ffmpeg, '-nostdin', '-y', '-v', 'error', *map(str, argv)],
                              capture_output=True, timeout=90)

    try:
        generated = ff('-f', 'lavfi', '-i', 'testsrc2=size=160x90:rate=10',
                       '-f', 'lavfi', '-i', 'sine=sample_rate=48000', '-t', '3',
                       '-c:v', 'libx264', '-g', '10', '-c:a', 'aac',
                       '-hls_time', '1', '-hls_list_size', '0',
                       '-hls_segment_filename', fixture / '%d.ts', fixture / 'base.m3u8')
        assert generated.returncode == 0, generated.stderr.decode(errors='replace')
        playlist = (fixture / 'base.m3u8').read_text()
        for suffix in ('jpeg', 'png', 'bin'):
            folder = fixture / suffix
            folder.mkdir(exist_ok=True)
            for segment in fixture.glob('*.ts'):
                shutil.copyfile(segment, folder / (segment.stem + '.' + suffix))
            (folder / 'index.m3u8').write_text(playlist.replace('.ts', '.' + suffix))
        bad = fixture / 'bad'
        bad.mkdir(exist_ok=True)
        (bad / 'index.m3u8').write_text('#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1,\nx.jpeg\n#EXT-X-ENDLIST\n')
        (bad / 'x.jpeg').write_bytes(b'not video data' * 256)

        class Handler(SimpleHTTPRequestHandler):
            def __init__(self, *argv, **kw):
                super().__init__(*argv, directory=str(fixture), **kw)

            def log_message(self, *_):
                pass

            def do_GET(self):
                counts[self.path] += 1
                if self.path == '/jpeg/0.jpeg' and not gate.is_set():
                    requested.set()
                    gate.wait(40)
                super().do_GET()

        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        port = free_port()
        env = {**os.environ, 'HLS_V7_DATA_DIR': str(root / 'data'),
               'HLS_V7_DOWNLOAD_DIR': str(root / 'downloads'),
               'HLS_V7_PIPE': rf'\\.\pipe\HLSDownloader.suffix-{uuid.uuid4().hex}',
               'HLS_V7_CORE_TCP': '1', 'HLS_V7_CORE_BIND': f'127.0.0.1:{port}',
               'HLS_FFMPEG': ffmpeg}
        with (args.report.parent / 'suffix-core.log').open('wb') as log:
            process = subprocess.Popen([str(args.engine)], env=env, stdout=log, stderr=log,
                                       creationflags=subprocess.CREATE_NO_WINDOW)
        with connect(port) as ipc:
            send(ipc, {'type': 'store_settings', 'request_id': 1, 'values': {
                'legal_terms_accepted': True, 'legal_terms_version': '2026-08-06-cn-1',
                'download_complete_popup_enabled': False, 'download_progress_window_enabled': False,
                'av_scan_enabled': False, 'auto_retry_failed_max': 0}})

            def command(value):
                return send(ipc, {'type': 'command', 'request_id': 2, 'command': value})

            def create(suffix):
                response = command({'kind': 'create_task', 'spec': {
                    'url': f'http://127.0.0.1:{server.server_port}/{suffix}/index.m3u8',
                    'filename': suffix + '-video.mp4', 'resource_kind': 'hls', 'concurrency': 2,
                    'proxy': 'hls-downloader://direct-proxy'}})
                return next(e['event']['snapshot']['task_id'] for e in response['events']
                            if e['event']['kind'] == 'task_created')

            def settled(tid):
                return wait_task(ipc, tid, lambda t: t['status'] in ('failed', 'completed'), 90)

            def verify(task):
                assert task['status'] == 'completed', task.get('error_message')
                output = Path(task['output_path'])
                meta = json.loads(subprocess.check_output([
                    str(args.tools / 'ffprobe.exe'), '-v', 'error', '-show_streams',
                    '-show_format', '-of', 'json', str(output)], timeout=30))
                assert 'mp4' in meta['format']['format_name'], meta['format']
                assert {'video', 'audio'} <= {s['codec_type'] for s in meta['streams']}
                assert float(meta['format']['duration']) >= 2.9
                decoded = ff('-xerror', '-i', output, '-f', 'null', 'NUL')
                assert decoded.returncode == 0, decoded.stderr.decode(errors='replace')

            tid = create('jpeg')
            assert requested.wait(20), 'JPEG fixture not requested'
            task_dir = root / 'downloads' / '.hls-tasks' / tid
            blocked_output = task_dir / 'merged.mp4'
            blocked_output.mkdir()
            gate.set()
            failed = settled(tid)
            assert failed['status'] == 'failed', failed
            assert failed['error_code'] == 'MEDIA_MERGE_FAILED', failed
            assert failed['error_stage'] == 'merging', failed
            assert 'Error opening output' in failed['error_message'], failed['error_message']
            assert (task_dir / 'ffmpeg-merge.log').stat().st_size > 0
            segment_counts = {k: v for k, v in counts.items() if k.endswith('.jpeg')}
            blocked_output.rmdir()
            command({'kind': 'task_action', 'task_id': tid, 'action': 'retry'})
            verify(settled(tid))
            assert segment_counts == {k: v for k, v in counts.items() if k.endswith('.jpeg')}, 'Retry downloaded cached segments again'
            report['checks'].append('jpeg: real MP4 with audio/video; failed merge log; cached retry without segment downloads')
            for suffix in ('png', 'bin'):
                verify(settled(create(suffix)))
                report['checks'].append(suffix + ': MP4 format, audio/video and full decode')
            failed = settled(create('bad'))
            assert failed['status'] == 'failed' and failed['error_code'] == 'MEDIA_MERGE_FAILED', failed
            assert failed['error_stage'] == 'merging' and 'Output file does not contain any stream' in failed['error_message'], failed
            assert list((root / 'downloads' / '.hls-tasks' / failed['task_id'] / 'segments').iterdir())
            report['checks'].append('invalid media fails at merging, exposes stderr and retains segments')
        report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc()
    finally:
        gate.set()
        if process:
            process.terminate()
            process.wait(timeout=15)
        if server:
            server.shutdown()
            server.server_close()
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps(report, ensure_ascii=False), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--tools', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
