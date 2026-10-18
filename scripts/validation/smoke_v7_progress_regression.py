"""Guest-only acceptance for progress, burst speed, dragging and resumed downloads."""
from pathlib import Path
import argparse
import ctypes
from ctypes import wintypes
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import subprocess
import threading
import time
import traceback
import uuid
from urllib.request import Request, urlopen

import psutil
from smoke_v7_portable_app import Pipe, wait, runtime_processes
from smoke_v7_presenter_visual import user32, find_window


def run(runtime, report_path):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Run only in the acceptance VM'
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'checks': [], 'samples': {}}
    created = []
    server = None
    def save():
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
    def record(name, details=True):
        report['checks'].append({'name': name, 'actual': details})
        save()
        print(name, flush=True)
    def core(message):
        pipe = Pipe()
        try: return pipe.request(message)
        finally: pipe.close()
    def command(value): return core({'type': 'command', 'request_id': 1, 'command': value})
    def task(task_id): return next(t for t in core({'type': 'snapshot', 'request_id': 2})['tasks'] if t['task_id'] == task_id)
    def action(task_id, name): return command({'kind': 'task_action', 'task_id': task_id, 'action': name})
    try:
        for proc in runtime_processes(runtime):
            if proc['name'].lower() == 'hlsdownloader.exe': psutil.Process(proc['pid']).terminate()
        port = 18793
        token = uuid.uuid4().hex
        env = {**os.environ, 'HLS_UI_TEST_API': '1', 'HLS_UI_TEST_PORT': str(port), 'HLS_UI_TEST_TOKEN': token}
        subprocess.Popen([str(runtime / 'HLSDownloader.exe')], cwd=runtime, env=env)
        def ui(path, raw=False, payload=None):
            with urlopen(Request(f'http://127.0.0.1:{port}/{path}', data=json.dumps(payload).encode() if payload else None, headers={'X-HLS-Test-Token': token, 'Content-Type': 'application/json'}), timeout=10) as response:
                body = response.read()
                return body if raw else json.loads(body)
        def ready():
            try: return '已连接' in ui('state')['engineText']
            except OSError: return False
        wait(ready, 'Workbench not connected', 90)
        def dismiss_update():
            bounds = ui('state')['controlBounds'].get('dialog.secondary.稍后')
            if bounds:
                ui('action', payload={'type': 'click', 'x': (bounds[0]+bounds[2])//2, 'y': (bounds[1]+bounds[3])//2})
                wait(lambda: 'dialog.secondary.稍后' not in ui('state')['controlBounds'], 'Update notice did not dismiss', 10)
        media = report_path.parent / 'segment.ts'
        subprocess.run([str(runtime/'app/resources/ffmpeg.exe'), '-y', '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=25', '-t', '0.6', '-c:v', 'libx264', '-f', 'mpegts', str(media)], check=True)
        segment = media.read_bytes()
        payload = bytes(range(256)) * (32*1024)
        class Handler(BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'
            def log_message(self, *_): pass
            def do_HEAD(self):
                self.send_response(200)
                if self.path.startswith('/known'): self.send_header('Content-Length', str(len(payload)))
                self.send_header('Connection', 'close'); self.end_headers(); self.close_connection = True
            def do_GET(self):
                try:
                    if self.path.startswith('/vod'):
                        body = ('#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n' + ''.join(f'#EXTINF:0.6,\n/segment-{i}.ts\n' for i in range(20)) + '#EXT-X-ENDLIST\n').encode()
                        self.send_response(200); self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body); return
                    if self.path.startswith('/segment-'):
                        time.sleep(1)
                        self.send_response(200); self.send_header('Content-Length', str(len(segment))); self.end_headers(); self.wfile.write(segment); return
                    offset = int(self.headers.get('Range', 'bytes=0-').split('=')[1].split('-')[0])
                    known = self.path.startswith('/known')
                    self.send_response(206 if offset else 200)
                    if known:
                        self.send_header('Content-Length', str(len(payload)-offset)); self.send_header('Accept-Ranges', 'bytes')
                        if offset: self.send_header('Content-Range', f'bytes {offset}-{len(payload)-1}/{len(payload)}')
                    self.send_header('Connection', 'close'); self.end_headers(); self.close_connection = True
                    for at in range(offset, len(payload), 65536):
                        if not known and at == 2*1024*1024: time.sleep(5)
                        self.wfile.write(payload[at:at+65536]); self.wfile.flush(); time.sleep(.08)
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError): pass
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        base = f'http://127.0.0.1:{server.server_port}'
        for name, kind in [('known.bin', 'file'), ('unknown.bin', 'file'), ('vod.m3u8', 'hls')]:
            response = command({'kind': 'create_task', 'spec': {'url': base+'/'+name, 'filename': name, 'resource_kind': kind, 'concurrency': 1}})
            assert 'events' in response, response
            snapshot = next(e['event']['snapshot'] for e in response['events'] if e['event']['kind'] == 'task_created')
            tid = snapshot['task_id']; created.append(tid)
            wait(lambda: task(tid)['downloaded_bytes'] > 0, 'No download bytes', 30)
            dismiss_update()
            presenter = next(p for p in runtime_processes(runtime) if 'Presenter' in p['name'])
            hud = wait(lambda: find_window(presenter['pid'], '下载进度'), 'No progress window', 15)
            if name == 'known.bin':
                before = wintypes.RECT(); user32.GetWindowRect(hud, ctypes.byref(before))
                scale = user32.GetDpiForWindow(hud)/96
                user32.SetForegroundWindow(hud)
                x, y = before.left+round(90*scale), before.top+round(30*scale)
                user32.SetCursorPos(x, y); time.sleep(.15); user32.mouse_event(2, 0, 0, 0, 0)
                for i in range(1, 11): user32.SetCursorPos(x+10*i, y+6*i); time.sleep(.04)
                user32.mouse_event(4, 0, 0, 0, 0); time.sleep(.3)
                after = wintypes.RECT(); user32.GetWindowRect(hud, ctypes.byref(after))
                report['drag_window']={'before':[before.left,before.top,before.right,before.bottom], 'after':[after.left,after.top,after.right,after.bottom], 'foreground_hwnd':user32.GetForegroundWindow(), 'hud_hwnd':hud}
                save()
                assert after.left-before.left > 60 and after.top-before.top > 30, 'Progress title drag did not move window'
                record('real-pointer-progress-title-drag', {'dx': after.left-before.left, 'dy': after.top-before.top})
                from PIL import ImageGrab
                drag_image=ImageGrab.grab(bbox=(after.left, after.top, after.right, after.bottom), include_layered_windows=True, all_screens=True).convert('RGB')
                drag_image.save(report_path.with_name('dragged-window-before-pause.png'))
                red=[(x,y) for y in range(drag_image.height*3//4,drag_image.height) for x in range(drag_image.width//2,drag_image.width) if (lambda c:c[0]>150 and c[0]>c[1]+40 and c[0]>c[2]+40)(drag_image.getpixel((x,y)))]
                assert len(red)>30, 'Progress footer missing after caption drag'
                left,right=min(x for x,y in red),max(x for x,y in red)
                top,bottom=min(y for x,y in red),max(y for x,y in red)
                pause_x=after.left+(left+right)//2-(right-left+1)-round(8*scale)
                pause_y=after.top+(top+bottom)//2
                report['pause_pointer']={'x':pause_x,'y':pause_y,'rect':[after.left,after.top,after.right,after.bottom]}; save()
                user32.SetForegroundWindow(hud)
                user32.SetCursorPos(pause_x, pause_y); time.sleep(.3)
                user32.mouse_event(2, 0, 0, 0, 0); time.sleep(.12); user32.mouse_event(4, 0, 0, 0, 0)
                wait(lambda: task(tid)['status']=='paused' and task(tid)['speed_bytes_per_sec']==0, 'Pause did not reset rate', 10)
                time.sleep(1); resumed_bytes = task(tid)['downloaded_bytes']; action(tid, 'resume')
                record('actual-progress-pause-button-preserves-bytes-and-resume', {'bytes': resumed_bytes})
            samples=[]; began=time.monotonic(); captured=False
            while time.monotonic()-began < 90:
                current = task(tid)
                samples.append({k: current.get(k) for k in ('status', 'downloaded_bytes', 'total_bytes', 'speed_bytes_per_sec', 'completed_ranges', 'total_ranges')})
                if current['status']=='failed': raise AssertionError(current.get('error_message'))
                ready_to_capture = current['downloaded_bytes'] > 262144 if kind == 'file' else current['completed_ranges'] >= 2
                if not captured and current['status'] == 'downloading' and ready_to_capture:
                    dismiss_update()
                    wait(lambda: 'taskrow.'+tid in ui('state')['controlBounds'], 'Main window omitted active task', 10)
                    report_path.with_name(name+'-main.png').write_bytes(ui('screenshot', True))
                    from PIL import ImageGrab
                    user32.SetWindowPos(hud, wintypes.HWND(-1), 0, 0, 0, 0, 0x0001|0x0002|0x0040)
                    user32.SetForegroundWindow(hud); time.sleep(.2)
                    rect=wintypes.RECT(); user32.GetWindowRect(hud, ctypes.byref(rect))
                    hud_image=ImageGrab.grab(bbox=(rect.left, rect.top, rect.right, rect.bottom), include_layered_windows=True, all_screens=True).convert('RGB')
                    hud_image.save(report_path.with_name(name+'-hud.png'))
                    lower=hud_image.crop((hud_image.width//2, hud_image.height*3//4, hud_image.width, hud_image.height))
                    red_pixels=sum(r>150 and r>g+40 and r>b+40 for r,g,b in lower.get_flattened_data())
                    assert red_pixels>30, 'Progress footer/cancel button disappeared after dragging or showing'
                    record(name+'-full-progress-window-rendered', {'cancel_button_pixels':red_pixels})
                    captured=True
                if current['status']=='completed': break
                time.sleep(.2)
            assert current['status']=='completed' and captured, current
            active=[s for s in samples if s['status']=='downloading' and s['downloaded_bytes']>0]
            report['samples'][name]=samples
            if kind=='hls':
                assert current['total_ranges'] == 20, 'Completion lost media segment count'
                assert any(0<s['completed_ranges']<s['total_ranges'] and s['total_ranges']==20 for s in active), active
                assert all(s['speed_bytes_per_sec']>0 for s in active[5:]), 'Rate flickers to zero between 1s segments'
            else:
                assert hashlib.sha256(Path(current['output_path']).read_bytes()).digest()==hashlib.sha256(payload).digest()
                assert max(s['speed_bytes_per_sec'] for s in active)<2*1024*1024, 'Resumed bytes inflated rate'
                if name=='unknown.bin': assert all(s['total_bytes'] in (None, 0) for s in active), 'Invented total for unknown response'
                if name=='unknown.bin':
                    from itertools import groupby
                    plateaus=[list(group) for _,group in groupby(active, key=lambda s:s['downloaded_bytes'])]
                    assert any(len(group)>=15 and any(s['speed_bytes_per_sec']==0 for s in group) for group in plateaus), 'Stalled transfer keeps displaying old speed'
            assert current['completed_ranges']==current['total_ranges'] and current['speed_bytes_per_sec']==0, current
            report['samples'][name]=samples
            record(name+'-accurate-progress-rate-and-completion', {'bytes':current['downloaded_bytes'], 'ranges':current['total_ranges']})
        report['passed']=True
    except Exception:
        report['error']=traceback.format_exc()
        print(report['error'], flush=True)
    finally:
        if server: server.shutdown()
        for tid in created:
            try:
                current=task(tid)
                if current['status'] not in ('completed', 'failed', 'canceled'): action(tid, 'cancel')
            except Exception: pass
        save()
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args=parser.parse_args()
    raise SystemExit(run(args.runtime.resolve(), args.report.resolve()))
