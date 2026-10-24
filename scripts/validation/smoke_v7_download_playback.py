"""Sandbox acceptance: real media, incomplete seeks, final-file switch and cache deletion."""
from __future__ import annotations

import argparse
import base64
import ctypes
from ctypes import wintypes
import hashlib
import io
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import threading
import time
import traceback
import uuid

import psutil
from PIL import Image

from msi_checkpoint_fixture import connect, send, wait_task


def wait(predicate, message, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.1)
    raise AssertionError(message)


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    args.report.parent.mkdir(parents=True, exist_ok=True)
    root = Path(r'C:\QAWork') / ('download-playback-' + uuid.uuid4().hex)
    fixture = root / 'fixtures'
    fixture.mkdir(parents=True)
    resources = args.runtime / 'app/resources'
    engine = resources / 'HLSDownloaderEngine.exe'
    report = {'passed': False, 'checks': [], 'requests': [], 'performance': {},
              'engine_sha256': hashlib.sha256(engine.read_bytes()).hexdigest()}
    process = server = None
    accelerated = threading.Event()
    audio_tail_release = threading.Event()
    audio_tail_release.set()

    def save():
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    def record(name, actual=True):
        report['checks'].append({'name': name, 'actual': actual})
        save()
        print(name, flush=True)

    def ff(*argv, cwd=None):
        result = subprocess.run([str(resources/'ffmpeg.exe'), '-nostdin', '-y', '-v', 'error',
                                 *map(str, argv)], cwd=cwd, capture_output=True, timeout=180)
        assert result.returncode == 0, result.stderr.decode(errors='replace')

    def player_windows():
        user = ctypes.windll.user32
        callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        user.EnumWindows.argtypes = [callback, wintypes.LPARAM]
        user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
        user.IsWindowVisible.argtypes = [wintypes.HWND]
        user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
        children = {p.pid for p in psutil.Process(process.pid).children(recursive=True)}
        result = []

        @callback
        def visit(hwnd, _):
            pid = wintypes.DWORD()
            user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if pid.value in children and user.IsWindowVisible(hwnd):
                rect = wintypes.RECT()
                user.GetWindowRect(hwnd, ctypes.byref(rect))
                result.append({'hwnd': hwnd, 'pid': pid.value,
                               'rect': [rect.left, rect.top, rect.right, rect.bottom]})
            return True
        user.EnumWindows(visit, 0)
        return result

    def measure(name, seconds=3):
        def sample():
            parent = psutil.Process(process.pid)
            result = {}
            for proc in [parent, *parent.children(recursive=True)]:
                try:
                    cpu = proc.cpu_times()
                    mem = proc.memory_info()
                    result[proc.pid] = {'name': proc.name(), 'cpu': cpu.user+cpu.system,
                                        'rss': mem.rss, 'private': getattr(mem, 'private', None)}
                except psutil.NoSuchProcess:
                    pass
            return result
        before = sample()
        started = time.monotonic()
        time.sleep(seconds)
        after = sample()
        elapsed = time.monotonic()-started
        cpu = sum(max(0, value['cpu']-before.get(pid, value)['cpu']) for pid, value in after.items())
        report['performance'][name] = {'elapsed_seconds': elapsed,
            'cpu_one_core_percent': cpu/elapsed*100,
            'cpu_machine_percent': cpu/elapsed*100/psutil.cpu_count(),
            'rss_sum_bytes': sum(v['rss'] for v in after.values()), 'processes': after}
        save()

    try:
        source = fixture/'source.mp4'
        ff('-f', 'lavfi', '-i', 'testsrc2=size=640x360:rate=25', '-f', 'lavfi', '-i',
           'sine=sample_rate=48000', '-t', '60', '-c:v', 'libx264', '-preset', 'ultrafast',
           '-b:v', '3M', '-minrate', '3M', '-maxrate', '3M', '-bufsize', '6M',
           '-x264-params', 'nal-hrd=cbr', '-g', '25', '-c:a', 'aac', '-movflags', '+faststart', source)
        for name in ('ts', 'fmp4', 'aes', 'external', 'dash'):
            (fixture/name).mkdir()
        for name in ('ts', 'fmp4', 'aes'):
            options = []
            extension = 'm4s' if name == 'fmp4' else 'jpeg'
            if name == 'fmp4':
                options += ['-hls_segment_type', 'fmp4']
            if name == 'aes':
                key = fixture/name/'key.bin'
                key.write_bytes(bytes(range(16)))
                info = fixture/name/'key-info.txt'
                info.write_text('key.bin\n'+str(key)+'\n'+'00'*15+'01\n')
                options += ['-hls_key_info_file', info]
            ff('-i', source, '-c', 'copy', '-hls_time', '1', '-hls_list_size', '0',
               *options, '-hls_segment_filename', f's%03d.{extension}', 'index.m3u8', cwd=fixture/name)
        external = fixture/'external'
        ff('-i', source, '-an', '-c:v', 'copy', '-hls_time', '1', '-hls_list_size', '0',
           '-hls_segment_filename', 'v%03d.ts', 'video.m3u8', cwd=external)
        ff('-i', source, '-vn', '-c:a', 'copy', '-hls_time', '1', '-hls_list_size', '0',
           '-hls_segment_filename', 'a%03d.ts', 'audio.m3u8', cwd=external)
        (external/'index.m3u8').write_text('#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",'
            'NAME="Audio",DEFAULT=YES,AUTOSELECT=YES,URI="audio.m3u8"\n'
            '#EXT-X-STREAM-INF:BANDWIDTH=3500000,AUDIO="a"\nvideo.m3u8\n')
        ff('-i', source, '-c', 'copy', '-f', 'dash', '-seg_duration', '1', 'index.mpd', cwd=fixture/'dash')

        class Handler(SimpleHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def __init__(self, *argv, **kw):
                super().__init__(*argv, directory=str(fixture), **kw)

            def log_message(self, *_):
                pass

            def do_GET(self):
                report['requests'].append({'path': self.path, 'range': self.headers.get('Range'),
                                           'at': time.monotonic()})
                audio = re.search(r'/external/a(\d+)\.ts', self.path)
                dash_audio = re.search(r'/dash/chunk-stream1-(\d+)\.m4s', self.path)
                if ((audio and int(audio[1]) > 0) or (dash_audio and int(dash_audio[1]) > 1)):
                    # 确定性保留未下载的音轨尾部，避免小样本全下完也误判为提前播放。
                    if not audio_tail_release.wait(60):
                        self.send_error(503, 'Audio prefix acceptance was not released')
                        return
                if self.path == '/source.mp4':
                    size = source.stat().st_size
                    value = self.headers.get('Range')
                    start, end = 0, size-1
                    if value:
                        first, _, last = value.removeprefix('bytes=').partition('-')
                        start, end = int(first or 0), min(int(last) if last else size-1, size-1)
                    self.send_response(206 if value else 200)
                    self.send_header('Content-Length', str(end-start+1))
                    self.send_header('Accept-Ranges', 'bytes')
                    self.send_header('ETag', '"playback-fixture"')
                    if value:
                        self.send_header('Content-Range', f'bytes {start}-{end}/{size}')
                    self.end_headers()
                    try:
                        with source.open('rb') as stream:
                            stream.seek(start)
                            remaining = end-start+1
                            while remaining:
                                body = stream.read(min(65536, remaining))
                                if not body:
                                    break
                                self.wfile.write(body)
                                self.wfile.flush()
                                remaining -= len(body)
                                if not accelerated.is_set():
                                    time.sleep(.12)
                    except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                        pass
                    return
                video_segment = (re.search(r'/[sv]\d+\.', self.path)
                                 or re.search(r'/chunk-stream0-\d+\.m4s', self.path))
                if video_segment and not accelerated.is_set():
                    time.sleep(.65)
                try:
                    super().do_GET()
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                    pass

        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        server.daemon_threads = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        env = {**os.environ, 'HLS_V7_DATA_DIR': str(root/'data'),
            'HLS_V7_DOWNLOAD_DIR': str(root/'downloads'), 'HLS_V7_CORE_TCP': '1',
            'HLS_V7_CORE_BIND': f'127.0.0.1:{port}', 'HLS_FFMPEG': str(resources/'ffmpeg.exe'),
            'HLS_V7_PIPE': rf'\\.\pipe\HLSPlayback-{uuid.uuid4().hex}'}
        log = args.report.with_suffix('.core.log').open('wb')
        process = subprocess.Popen([str(engine)], env=env, stdout=log, stderr=log,
                                   creationflags=subprocess.CREATE_NO_WINDOW)
        with connect(port) as ipc:
            def command(value):
                response = send(ipc, {'type': 'command', 'request_id': 2, 'command': value})
                assert 'events' in response, response
                return response

            def setting(values):
                return send(ipc, {'type': 'store_settings', 'request_id': 1, 'values': values})

            def tasks():
                return send(ipc, {'type': 'snapshot', 'request_id': 3})['tasks']

            def player(action='status'):
                value = command({'kind': 'player_control', 'action': action})
                return next(e['event'] for e in value['events'] if e['event']['kind'] == 'player_session')

            def create(path, kind, filename):
                value = command({'kind': 'create_task', 'spec': {
                    'url': f'http://127.0.0.1:{server.server_port}/{path}',
                    'filename': filename, 'resource_kind': kind, 'concurrency': 1,
                    'proxy': 'hls-downloader://direct-proxy'}})
                created = next((e['event']['snapshot']['task_id'] for e in value['events']
                                if e['event']['kind'] == 'task_created'), None)
                assert created, value
                return created

            setting({'legal_terms_accepted': True, 'legal_terms_version': '2026-08-06-cn-1',
                     'download_complete_popup_enabled': False, 'download_progress_window_enabled': False,
                     'av_scan_enabled': False, 'auto_retry_failed_max': 0})
            measure('core_idle')
            for name, path, kind in [('ts', 'ts/index.m3u8', 'hls'),
                    ('fmp4', 'fmp4/index.m3u8', 'hls'), ('aes', 'aes/index.m3u8', 'hls'),
                    ('external', 'external/index.m3u8', 'hls'), ('dash', 'dash/index.mpd', 'dash'),
                    ('http', 'source.mp4', 'file')]:
                accelerated.clear()
                if name in ('external', 'dash'):
                    audio_tail_release.clear()
                start_requests = len(report['requests'])
                tid = create(path, kind, name+'.mp4')
                # 预览清单与合并后的进度事件分别发布；续传验收须先等到非零检查点。
                task = wait_task(ipc, tid, lambda t: (t['playback_ready']
                    and t['downloaded_bytes'] > 0) or t['status'] == 'failed', 90)
                assert task['status'] not in ('failed', 'completed'), task
                work = root/'downloads/.hls-tasks'/tid
                if name in ('external', 'dash'):
                    audio_playlist = work/'audio'/('preview-video.m3u8' if name == 'external' else 'preview-audio.m3u8')
                    media = [line.strip() for line in audio_playlist.read_text().splitlines()
                             if line.strip() and not line.startswith('#')]
                    tail = audio_playlist.parent/media[-1]
                    assert not tail.exists(), f'{name}: preview only opened after the complete audio track'
                    record(name+': preview opens while audio tail is blocked and absent')
                    audio_tail_release.set()
                command({'kind': 'task_action', 'task_id': tid, 'action': 'pause'})
                paused = wait_task(ipc, tid, lambda t: t['status'] == 'paused'
                                   and t['active_workers'] == 0, 30)
                assert work.is_dir() and paused['downloaded_bytes'] > 0, paused
                command({'kind': 'task_action', 'task_id': tid, 'action': 'resume'})
                task = wait_task(ipc, tid, lambda t: (t['status'] == 'downloading'
                    and t['playback_ready']) or t['status'] in ('failed', 'completed'), 60)
                assert task['status'] == 'downloading', task
                record(name+': pause preserves cache and resume restores playable download')
                if name == 'ts':
                    measure('downloading')
                command({'kind': 'play_task', 'task_id': tid})
                initial = wait(lambda: (p if (p := player())['position_available'] else None),
                               f'{name}: player did not decode', 60)
                assert abs(initial['duration_seconds']-60) < 1, initial
                wait(player_windows, 'No real player window')
                player('pause')
                player('speed:1.5')
                player('seek_to:48')
                seek = wait(lambda: (p if (p := player())['position_available']
                             and abs(p['position_seconds']-48) < 2 else None),
                            f'{name}: could not seek into unfinished download', 60)
                assert seek['paused'] is True, seek
                still = next(t for t in tasks() if t['task_id'] == tid)
                assert still['status'] != 'completed', 'Seek was only tested after completion'
                work = root/'downloads/.hls-tasks'/tid
                priority = work/('playback-byte-priority' if name == 'http' else 'playback-priority')
                assert priority.is_file(), f'{name}: seek did not request download priority'
                player('seek_to:12')
                wait(lambda: abs(player()['position_seconds']-12) < 2,
                     f'{name}: could not seek back into earlier media', 60)
                player('seek_to:48')
                seek = wait(lambda: (p if (p := player())['position_available']
                             and abs(p['position_seconds']-48) < 2 else None),
                            f'{name}: repeated seek did not return to 80%', 60)
                player('resume')
                wait(lambda: player()['position_seconds'] > seek['position_seconds']+.3,
                     f'{name}: seek changed the clock but playback remained stalled', 60)
                if name == 'ts':
                    measure('downloading_and_playing')
                before_finish = player('pause')
                # 悬停缩略图必须独立解码，不能暂停、跳转或重置正在播放的会话。
                thumbnail_before = player()
                thumbnail = send(ipc, {'type': 'player_thumbnail', 'request_id': 41,
                                      'task_id': tid, 'seconds': 0.0})
                assert thumbnail['type'] == 'player_thumbnail', thumbnail
                with Image.open(io.BytesIO(base64.b64decode(thumbnail['jpeg_base64']))) as frame:
                    assert 0 < frame.width <= 184 and 0 < frame.height <= 104, frame.size
                thumbnail_after = player()
                assert thumbnail_after['paused'] is True, thumbnail_after
                assert abs(thumbnail_after['position_seconds']-thumbnail_before['position_seconds']) < .15
                assert abs(thumbnail_after['speed']-thumbnail_before['speed']) < .01
                assert not any('image2pipe' in p.cmdline() for p in psutil.Process(process.pid).children(recursive=True))
                record(name+': thumbnail leaves playback position, pause and speed unchanged')
                record(name+': unfinished download decodes, exposes full duration and seeks to 80%', seek)
                hwnd = player_windows()
                accelerated.set()
                completed = wait_task(ipc, tid, lambda t: t['status'] in ('completed', 'failed'), 120)
                assert completed['status'] == 'completed', completed
                wait(lambda: not work.exists(), f'{name}: cache remains after completion', 20)
                final = wait(lambda: (p if (p := player())['position_available'] else None),
                             f'{name}: final file not loaded')
                assert abs(final['position_seconds']-before_finish['position_seconds']) < .5, (before_finish, final)
                assert final['paused'] and abs(final['speed']-1.5) < .01, final
                final_window = player_windows()
                report['window_switch'] = {'before': hwnd, 'after': final_window}
                save()
                assert final_window == hwnd, 'Completion replaced/moved the player window'
                thumbnail = send(ipc, {'type': 'player_thumbnail', 'request_id': 42,
                                      'task_id': tid, 'seconds': 12.0})
                assert thumbnail['type'] == 'player_thumbnail', thumbnail
                with Image.open(io.BytesIO(base64.b64decode(thumbnail['jpeg_base64']))) as frame:
                    assert frame.format == 'JPEG'
                output = Path(completed['output_path'])
                ff('-xerror', '-i', output, '-f', 'null', 'NUL')
                digest = hashlib.sha256(output.read_bytes()).hexdigest()
                player('resume')
                wait(lambda: player()['position_seconds'] > final['position_seconds']+.5,
                     f'{name}: playback no longer advances after cleanup')
                player('stop')
                wait(lambda: not player_windows(), 'Close left the player visible')
                command({'kind': 'task_action', 'task_id': tid, 'action': 'delete'})
                assert hashlib.sha256(output.read_bytes()).hexdigest() == digest
                record(name+': same-window final switch; complete decode; cache removed; delete preserves output',
                       {'window': hwnd, 'final': final, 'output_sha256': digest,
                        'origin_requests': report['requests'][start_requests:]})

            accelerated.clear()
            pending = create('ts/index.m3u8', 'hls', 'unfinished.mp4')
            wait_task(ipc, pending, lambda t: t['downloaded_bytes'] > 0, 30)
            command({'kind': 'task_action', 'task_id': pending, 'action': 'pause'})
            work = root/'downloads/.hls-tasks'/pending
            assert work.is_dir(), 'Pause destroyed resumable fragments'
            sentinel = root/'downloads/unrelated.txt'
            sentinel.write_text('untouched')
            command({'kind': 'task_action', 'task_id': pending, 'action': 'delete'})
            assert not work.exists(), 'Deleting an unfinished task retained its cache'
            assert sentinel.read_text() == 'untouched'
            record('pause retains resumable cache; deleting unfinished task cleans cache and preserves unrelated file')

            accelerated.clear()
            closed = create('ts/index.m3u8', 'hls', 'closed-before-finish.mp4')
            wait_task(ipc, closed, lambda t: t['playback_ready'], 30)
            command({'kind': 'play_task', 'task_id': closed})
            wait(lambda: player()['position_available'], 'Close fixture did not start playing')
            player('stop')
            wait(lambda: not player_windows(), 'Player did not close during download')
            accelerated.set()
            completed = wait_task(ipc, closed, lambda t: t['status'] in ('completed', 'failed'), 90)
            assert completed['status'] == 'completed', completed
            work = root/'downloads/.hls-tasks'/closed
            wait(lambda: not work.exists(), 'Closed player prevented cache cleanup')
            assert not player_windows() and not player()['active'], 'Completion reopened a closed player'
            command({'kind': 'task_action', 'task_id': closed, 'action': 'delete'})
            record('closing playback during download stays closed after completion and allows cleanup')

            accelerated.set()
            setting({'keep_temp_files': True})
            kept = create('ts/index.m3u8', 'hls', 'kept.mp4')
            completed = wait_task(ipc, kept, lambda t: t['status'] in ('completed', 'failed'), 90)
            assert completed['status'] == 'completed', completed
            output = Path(completed['output_path'])
            work = root/'downloads/.hls-tasks'/kept
            assert (work/'segments').is_dir()
            command({'kind': 'clear_completed'})
            assert not work.exists() and output.is_file()
            record('keep_temp_files opt-in works; clear-completed still cleans cache and preserves output')
        report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc()
    finally:
        accelerated.set()
        audio_tail_release.set()
        if process:
            for child in psutil.Process(process.pid).children(recursive=True):
                try:
                    child.terminate()
                except psutil.NoSuchProcess:
                    pass
            process.terminate()
            process.wait(timeout=15)
        if server:
            server.shutdown()
            server.server_close()
        save()
        print(json.dumps({k: v for k, v in report.items() if k != 'requests'}, ensure_ascii=False), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
