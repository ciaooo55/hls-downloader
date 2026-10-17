"""Sandbox regression: Compose controls must change the real mpv session."""
from __future__ import annotations

import argparse
import base64
import ctypes
from ctypes import wintypes
import functools
from http.client import RemoteDisconnected
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import io
import os
from pathlib import Path
import socket
import subprocess
import threading
import time
import traceback
from urllib.request import Request, urlopen
import uuid

import psutil
from PIL import Image, ImageChops, ImageStat

from msi_checkpoint_fixture import connect, send, wait_task


def wait(predicate, message, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.15)
    raise AssertionError(message)


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    root = Path(r'C:\QAWork') / ('overlay-' + uuid.uuid4().hex)
    root.mkdir(parents=True)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'checks': [], 'requested_java_ui_scale': args.ui_scale}
    processes = []
    resources = args.runtime / 'app/resources'
    port, main_port, pip_port = free_port(), free_port(), free_port()
    token = uuid.uuid4().hex
    active_port = main_port
    env = {**os.environ, 'HLS_V7_DATA_DIR': str(root/'data'),
           'HLS_V7_DOWNLOAD_DIR': str(root/'downloads'), 'HLS_V7_CORE_TCP': '1',
           'HLS_V7_CORE_BIND': f'127.0.0.1:{port}', 'HLS_UI_TEST_API': '1',
           'HLS_UI_TEST_PORT': str(main_port), 'HLS_UI_TEST_PIP_PORT': str(pip_port),
           'HLS_UI_TEST_TOKEN': token}
    env.pop('HLS_V7_PIPE', None)
    if args.ui_scale is not None:
        env['JAVA_TOOL_OPTIONS'] = env.get('JAVA_TOOL_OPTIONS', '') + f' -Dsun.java2d.uiScale={args.ui_scale}'
    log = args.report.with_suffix('.runtime.log').open('wb')

    class Handler(SimpleHTTPRequestHandler):
        def log_message(self, *_):
            pass
    server = ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Handler, directory=str(args.media.parent)))
    threading.Thread(target=server.serve_forever, daemon=True).start()

    def save():
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    def record(name):
        report['checks'].append(name)
        save()
        print(name, flush=True)

    def ui(path, values=None, raw=False):
        data = json.dumps(values).encode() if values is not None else None
        with urlopen(Request(f'http://127.0.0.1:{active_port}/{path}', data=data,
                     headers={'X-HLS-Test-Token': token, 'Content-Type': 'application/json'}), timeout=10) as response:
            body = response.read()
            return body if raw else json.loads(body)

    def action(kind, **values):
        return ui('action', {'type': kind, **values})

    def bounds(name):
        return wait(lambda: ui('state')['controlBounds'].get('video.'+name), 'Missing '+name, 15)

    def click(name):
        left, top, right, bottom = bounds(name)
        action('click', x=(left+right)//2, y=(top+bottom)//2)

    def capture(name):
        args.report.with_name(args.report.stem+'-'+name+'.png').write_bytes(ui('screenshot', raw=True))

    def check_video_rect(name):
        # 同时核对 mpv 自己的子窗口，不能只验证外层 STATIC 宿主的尺寸。
        user = ctypes.windll.user32
        user.SetThreadDpiAwarenessContext.argtypes = [ctypes.c_void_p]
        user.SetThreadDpiAwarenessContext.restype = ctypes.c_void_p
        previous = user.SetThreadDpiAwarenessContext(ctypes.c_void_p(-4))
        callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        user.EnumWindows.argtypes = [callback, wintypes.LPARAM]
        user.EnumChildWindows.argtypes = [wintypes.HWND, callback, wintypes.LPARAM]
        user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
        user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
        user.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
        native_pids = {p.pid for p in psutil.Process(processes[0].pid).children(recursive=True)}
        found = []

        @callback
        def child(hwnd, _):
            pid = wintypes.DWORD()
            user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if pid.value in native_pids:
                rect = wintypes.RECT()
                user.GetWindowRect(hwnd, ctypes.byref(rect))
                found.append([rect.left, rect.top, rect.right, rect.bottom])
            return True

        @callback
        def host(hwnd, _):
            title = ctypes.create_unicode_buffer(256)
            user.GetWindowTextW(hwnd, title, len(title))
            expected_pip = active_port == pip_port
            if title.value.startswith('HLS Downloader') and title.value.endswith('播放器') == expected_pip:
                user.EnumChildWindows(hwnd, child, 0)
            return True

        try:
            def aligned():
                found.clear()
                window = ui('window')
                left, top, right, bottom = bounds('surface')
                expected = [window['deviceX']+left, window['deviceY']+top,
                            window['deviceX']+right, window['deviceY']+bottom]
                user.EnumWindows(host, 0)
                report.setdefault('video_rectangles', {})[name] = {'expected': expected, 'actual': list(found)}
                return len(found) >= 2 and all(all(abs(a-b) <= 3 for a, b in zip(rect, expected)) for rect in found)
            wait(aligned, 'Video windows did not align: '+name, 10)
        finally:
            user.SetThreadDpiAwarenessContext(previous)

    try:
        processes.append(subprocess.Popen([str(resources/'HLSDownloaderEngine.exe')], env=env,
                         stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW))
        with connect(port) as ipc:
            send(ipc, {'type': 'store_settings', 'request_id': 1, 'values': {
                'legal_terms_accepted': True, 'legal_terms_version': '2026-08-06-cn-1',
                'download_complete_popup_enabled': False, 'download_progress_window_enabled': False,
                'av_scan_enabled': False, 'auto_retry_failed_max': 0}})

            def command(value):
                result = send(ipc, {'type': 'command', 'request_id': 2, 'command': value})
                assert 'events' in result, result
                return result

            def player():
                result = command({'kind': 'player_control', 'action': 'status'})
                return next(e['event'] for e in result['events'] if e['event']['kind'] == 'player_session')

            created = command({'kind': 'create_task', 'spec': {
                'url': f'http://127.0.0.1:{server.server_port}/{args.media.name}',
                'filename': 'overlay-multitrack.mkv', 'resource_kind': 'file',
                'concurrency': 1, 'proxy': 'hls-downloader://direct-proxy'}})
            tid = next(e['event']['snapshot']['task_id'] for e in created['events'] if e['event']['kind'] == 'task_created')
            task = wait_task(ipc, tid, lambda t: t['status'] in ('completed', 'failed'), 60)
            assert task['status'] == 'completed', task
            processes.append(subprocess.Popen([str(args.runtime/'HLSDownloader.exe')], cwd=args.runtime,
                             env=env, stdout=log, stderr=log))

            def ready():
                try:
                    return '已连接' in ui('state').get('engineText', '')
                except OSError:
                    return False
            wait(ready, 'UI did not connect', 120)
            report['window'] = ui('window')
            wait(lambda: 'taskrow.'+tid in ui('state')['controlBounds'], 'Task row missing')

            def open_player():
                action('context_menu_action', index=0, context_action='play')
                bounds('surface')
                wait(lambda: player()['position_available'], 'UI did not start playback')
            open_player()
            click('暂停')
            wait(lambda: player()['paused'], 'Pause did not reach mpv')
            check_video_rect('main')
            capture('main')
            record('main-window overlay starts real playback and pause button pauses it')
            click('播放')
            wait(lambda: not player()['paused'], 'Resume did not reach mpv')
            click('surface')
            wait(lambda: player()['paused'], 'Video click did not pause')
            record('resume and video-surface click update real pause state')
            before = player()['position_seconds']
            click('前进 10 秒')
            wait(lambda: abs(player()['position_seconds']-before-10) < 1, 'Forward button failed')
            click('后退 10 秒')
            wait(lambda: abs(player()['position_seconds']-before) < 1, 'Back button failed')
            left, top, right, bottom = bounds('timeline')
            action('drag', x=left+(right-left)//5, y=(top+bottom)//2,
                   to_x=left+int((right-left)*.6), to_y=(top+bottom)//2)
            wait(lambda: abs(player()['position_seconds']/player()['duration_seconds']-.6) < .07, 'Timeline drag failed')
            record('forward/back buttons and timeline drag seek the actual decoder')
            left, top, right, bottom = bounds('volume')
            action('click', x=left+(right-left)//3, y=(top+bottom)//2)
            wait(lambda: 20 < player()['volume'] < 45, 'Volume slider failed')
            click('静音')
            wait(lambda: player()['muted'], 'Mute failed')
            click('取消静音')
            wait(lambda: not player()['muted'], 'Unmute failed')
            record('volume and mute controls change mpv volume/mute')
            click('rate')
            click('rate.1.5')
            wait(lambda: abs(player()['speed']-1.5) < .01, 'Rate selection failed')
            for label, kind in [('音轨', 'audio'), ('字幕', 'sub')]:
                tracks = [t for t in player()['tracks'] if t['kind'] == kind]
                assert len(tracks) >= 2, tracks
                click(label)
                click(label+'.'+str(tracks[-1]['id']))
                wait(lambda: any(t['id'] == tracks[-1]['id'] and t['kind'] == kind and t['selected']
                                 for t in player()['tracks']), label+' selection failed')
            click('字幕')
            click('字幕.no')
            wait(lambda: not any(t['kind'] == 'sub' and t['selected'] for t in player()['tracks']), 'Subtitle off failed')
            record('rate, alternate audio/subtitle selection and subtitle-off affect real tracks')
            before = player()['position_seconds']
            left, top, right, bottom = bounds('timeline')
            hover_x = int((right-left)*.38)
            action('move', x=left+hover_x, y=(top+bottom)//2)
            time.sleep(1.5)
            capture('thumbnail')
            assert player()['paused'] and abs(player()['position_seconds']-before) < .2
            thumb = send(ipc, {'type': 'player_thumbnail', 'request_id': 20, 'task_id': tid, 'seconds': 20.0})
            assert thumb['type'] == 'player_thumbnail', thumb
            expected_image = Image.open(io.BytesIO(base64.b64decode(thumb['jpeg_base64']))).convert('RGB')
            screen = Image.open(args.report.with_name(args.report.stem+'-thumbnail.png')).convert('RGB')
            scale = ui('window')['scaleX']
            image_x = round((left+hover_x)/scale-92+(184-expected_image.width)/2)
            image_y = round(top/scale-136+(104-expected_image.height)/2)
            actual_image = screen.crop((image_x, image_y, image_x+expected_image.width, image_y+expected_image.height))
            difference = sum(ImageStat.Stat(ImageChops.difference(actual_image, expected_image)).mean)/3
            report['thumbnail_pixel_mean_error'] = difference
            assert difference < 25, f'Preview is obscured or misplaced: pixel error {difference}'
            record('hovering the preview preserves paused playback position')
            click('全屏')
            bounds('退出全屏')
            wait(lambda: (w := ui('window'))['width'] > report['window']['width']
                         or w['height'] > report['window']['height'], 'Fullscreen did not resize window', 10)
            check_video_rect('fullscreen')
            capture('fullscreen')
            action('key', key='ESCAPE')
            bounds('全屏')
            record('main fullscreen enters and Escape restores the overlay')
            click('画中画')
            active_port = pip_port
            wait(lambda: ui('window')['showing'], 'PiP not visible')
            bounds('返回主界面播放器')
            check_video_rect('pip')
            capture('pip')
            assert player()['paused'] and abs(player()['position_seconds']-before) < .2
            window = ui('window')
            left, top, right, bottom = bounds('header')
            action('drag', x=left+30, y=(top+bottom)//2, to_x=left+90, to_y=(top+bottom)//2+30)
            moved = ui('window')
            assert (window['x'], window['y']) != (moved['x'], moved['y']), (window, moved)
            click('播放')
            wait(lambda: not player()['paused'], 'PiP resume failed')
            click('暂停')
            wait(lambda: player()['paused'], 'PiP pause failed')
            try:
                click('返回主界面播放器')
            except RemoteDisconnected:
                # 返回操作会销毁 PiP 及其测试接口；以下继续验证主界面确实恢复。
                pass
            active_port = main_port
            bounds('画中画')
            check_video_rect('returned-main')
            capture('returned-main')
            left, top, right, bottom = bounds('surface')
            scale = ui('window')['scaleX']
            screen = Image.open(args.report.with_name(args.report.stem+'-returned-main.png')).convert('RGB')
            video = screen.crop(tuple(round(value/scale) for value in (left, top, right, bottom)))
            assert sum(ImageStat.Stat(video).mean) > 60, 'Returned main video is black'
            record('PiP preserves session, moves by title drag, plays/pauses and returns to main')
            click('关闭播放器，下载继续')
            wait(lambda: not player()['active'], 'Close button left playback active')
            wait(lambda: 'video.surface' not in ui('state')['controlBounds'], 'Closed overlay still visible')
            open_player()
            action('key', key='ESCAPE')
            wait(lambda: not player()['active'], 'Escape did not close reopened overlay')
            record('close button and Escape release playback; reopening works')
            report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc()
        try:
            report['ui_state'] = ui('state')
            capture('failure')
        except Exception:
            pass
    finally:
        for process in reversed(processes):
            try:
                parent = psutil.Process(process.pid)
                for child in parent.children(recursive=True):
                    child.terminate()
                parent.terminate()
                process.wait(timeout=15)
            except (psutil.NoSuchProcess, subprocess.TimeoutExpired):
                pass
        server.shutdown()
        server.server_close()
        log.close()
        save()
        print(json.dumps(report, ensure_ascii=False), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--media', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--ui-scale', type=float, choices=[1.0, 1.5])
    raise SystemExit(run(parser.parse_args()))
