"""Guest-only regression for HLS segments whose URL suffix is not their format."""
from pathlib import Path
import argparse
from collections import Counter
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import re
import shutil
import subprocess
import threading
import time
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
    report = {'passed': False, 'checks': [], 'script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'engine_sha256': hashlib.sha256(args.engine.read_bytes()).hexdigest()}
    process = server = None
    gate = threading.Event()
    requested = threading.Event()
    counts = Counter()
    ffmpeg = str(args.tools / 'ffmpeg.exe')

    def ff(*argv, cwd=None):
        return subprocess.run([ffmpeg, '-nostdin', '-y', '-v', 'error', *map(str, argv)],
                              capture_output=True, timeout=90, cwd=cwd)

    try:
        generated = ff('-f', 'lavfi', '-i', 'testsrc2=size=160x90:rate=10',
                       '-f', 'lavfi', '-i', 'sine=sample_rate=48000', '-t', '3',
                       '-c:v', 'libx264', '-g', '10', '-c:a', 'aac',
                       '-hls_time', '1', '-hls_list_size', '0',
                       '-hls_segment_filename', fixture / '%d.ts', fixture / 'base.m3u8')
        assert generated.returncode == 0, generated.stderr.decode(errors='replace')
        playlist = (fixture / 'base.m3u8').read_text()
        for suffix in ('jpeg', 'png', 'bin', 'keep', 'live'):
            folder = fixture / suffix
            folder.mkdir(exist_ok=True)
            for segment in fixture.glob('*.ts'):
                shutil.copyfile(segment, folder / (segment.stem + '.' + suffix))
            (folder / 'index.m3u8').write_text(playlist.replace('.ts', '.' + suffix))
        dash = fixture / 'dash'
        dash.mkdir()
        generated = ff('-f', 'lavfi', '-i', 'testsrc2=size=160x90:rate=10',
                       '-f', 'lavfi', '-i', 'sine=sample_rate=48000', '-t', '3',
                       '-c:v', 'libx264', '-g', '10', '-c:a', 'aac', '-f', 'dash',
                       '-seg_duration', '1', 'index.mpd', cwd=dash)
        assert generated.returncode == 0, generated.stderr.decode(errors='replace')
        subtitles = fixture / 'subtitles'
        subtitles.mkdir()
        (subtitles / 'index.m3u8').write_text(
            '#EXTM3U\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="English",LANGUAGE="en",URI="en.vtt"\n'
            '#EXT-X-STREAM-INF:BANDWIDTH=500000,SUBTITLES="subs"\n../base.m3u8\n')
        (subtitles / 'en.vtt').write_text('WEBVTT\n\n00:00:00.000 --> 00:00:02.500\nCleanup subtitle fixture\n')
        segmented = fixture / 'segmented-subtitles'
        segmented.mkdir()
        (segmented / 'index.m3u8').write_text(
            '#EXTM3U\n#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="English",LANGUAGE="en",URI="captions"\n'
            '#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="English CC",LANGUAGE="en",URI="../subtitles/en.vtt"\n'
            '#EXT-X-STREAM-INF:BANDWIDTH=500000,SUBTITLES="subs"\n../base.m3u8\n')
        (segmented / 'captions').write_text('#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:1.5,\nfirst.vtt\n#EXTINF:1.5,\nsecond.vtt\n#EXT-X-ENDLIST\n')
        (segmented / 'first.vtt').write_text('WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:0\n\n00:00:00.000 --> 00:00:01.000\nSegment one\n')
        (segmented / 'second.vtt').write_text('WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:135000\n\n00:00:00.000 --> 00:00:01.000\nSegment two\n')
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
            work_root = root / 'downloads'

            def command(value):
                return send(ipc, {'type': 'command', 'request_id': 2, 'command': value})

            def create(suffix, kind='hls', playlist_name='index.m3u8', filename=None):
                response = command({'kind': 'create_task', 'spec': {
                    'url': f'http://127.0.0.1:{server.server_port}/{suffix}/{playlist_name}',
                    'filename': filename or suffix + '-video.mp4', 'resource_kind': kind, 'concurrency': 2,
                    'proxy': 'hls-downloader://direct-proxy'}})
                return next(e['event']['snapshot']['task_id'] for e in response['events']
                            if e['event']['kind'] == 'task_created')

            def settled(tid):
                return wait_task(ipc, tid, lambda t: t['status'] in ('failed', 'completed'), 90)

            def verify(task, keep_temp=False):
                assert task['status'] == 'completed', task.get('error_message')
                output = Path(task['output_path'])
                # 生产包已移除 ffprobe；用已有 ffmpeg 检查真实容器并完整解码两种轨道。
                decoded = ff('-loglevel', 'info', '-xerror', '-i', output,
                             '-map', '0:v:0', '-map', '0:a:0', '-progress', 'pipe:1', '-f', 'null', 'NUL')
                assert decoded.returncode == 0, decoded.stderr.decode(errors='replace')
                assert re.search(r'Input #0, [^\r\n]*\bmp4\b', decoded.stderr.decode(errors='replace'))
                times = re.findall(rb'(?m)^out_time_us=(\d+)', decoded.stdout)
                assert times and max(map(int, times)) >= 2_900_000, decoded.stdout.decode(errors='replace')
                work = work_root / '.hls-tasks' / task['task_id']
                if keep_temp:
                    assert (work / 'segments').is_dir(), 'Opted-in segments were deleted'
                else:
                    deadline = time.monotonic() + 10
                    while work.exists() and time.monotonic() < deadline:
                        time.sleep(.05)
                    assert not work.exists(), f'Task cache or metadata remains: {work}'
                return output

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
            report['checks'].append('jpeg: MP4 full decode; failed merge preserves segments; retry reuses and then cleans segments')
            original = root / 'downloads' / 'jpeg-video.mp4'
            original_hash = hashlib.sha256(original.read_bytes()).hexdigest()
            renamed = verify(settled(create('png', filename='jpeg-video.mp4')))
            assert renamed.name == 'jpeg-video_1.mp4', renamed
            assert hashlib.sha256(original.read_bytes()).hexdigest() == original_hash
            report['checks'].append('png: full decode; renamed output still resolves; existing output preserved; cache cleaned')
            verify(settled(create('bin')))
            report['checks'].append('bin: MP4 format, audio/video, full decode and cache cleanup')
            verify(settled(create('live', kind='live')))
            report['checks'].append('ended live HLS: full decode and cache cleanup')
            subtitle_output = verify(settled(create('subtitles')))
            sidecars = list(subtitle_output.parent.glob(subtitle_output.stem + '.*.vtt'))
            assert sidecars and any('Cleanup subtitle fixture' in p.read_text() for p in sidecars)
            report['checks'].append('HLS direct WebVTT subtitles copied beside output before subtitle cache cleanup')
            subtitle_output = verify(settled(create('segmented-subtitles')))
            sidecars = list(subtitle_output.parent.glob(subtitle_output.stem + '.*.vtt'))
            assert len(sidecars) == 2, sidecars
            segmented_sidecar = next(p for p in sidecars if 'Segment one' in p.read_text())
            assert any('Cleanup subtitle fixture' in p.read_text() for p in sidecars), sidecars
            text = segmented_sidecar.read_text()
            assert 'Segment one' in text and 'Segment two' in text and '00:00:01.500 --> 00:00:02.500' in text, text
            srt = segmented_sidecar.with_suffix('.srt')
            assert srt.is_file() and '00:00:01,500 --> 00:00:02,500' in srt.read_text()
            report['checks'].append('HLS extensionless segmented WebVTT merged with timestamp maps; same-language tracks preserved as separate VTT/SRT sidecars and cache cleaned')
            send(ipc, {'type': 'store_settings', 'request_id': 3, 'values': {'keep_temp_files': True}})
            verify(settled(create('keep')), keep_temp=True)
            report['checks'].append('keep_temp_files=true preserves media segments')
            work_root = root / 'custom-work'
            send(ipc, {'type': 'store_settings', 'request_id': 4, 'values': {
                'keep_temp_files': False, 'temp_dir': str(work_root)}})
            verify(settled(create('dash', kind='dash', playlist_name='index.mpd')))
            report['checks'].append('DASH: custom work directory; full decode; video/audio fragments and merged intermediates cleaned')
            failed = settled(create('bad'))
            assert failed['status'] == 'failed' and failed['error_code'] == 'MEDIA_MERGE_FAILED', failed
            assert failed['error_stage'] == 'merging' and 'Output file does not contain any stream' in failed['error_message'], failed
            assert list((work_root / '.hls-tasks' / failed['task_id'] / 'segments').iterdir())
            report['checks'].append('invalid media fails at merging, exposes stderr and retains segments')
        report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc()
        report['requests'] = dict(counts)
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
