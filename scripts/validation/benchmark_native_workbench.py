"""Measure actual redraw cadence of the packaged software-rendered workbench."""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
import traceback
import uuid

from smoke_v7_resource_boundaries import Core, seeded_progress, wait
from soak_v7_runtime import percentile
from validation_workspace import test_tmp


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--seconds', type=int, default=20)
    args = parser.parse_args()
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.name != 'nt':
        raise RuntimeError('Workbench benchmark requires an isolated hosted Windows desktop')
    from pywinauto import Desktop
    runtime = args.runtime.resolve()
    engine = runtime/'app/resources/HLSDownloaderEngine.exe'
    presenter = runtime/'app/resources/HLSDownloaderPresenter.exe'
    root = test_tmp()/('native-frame-benchmark-' + uuid.uuid4().hex)
    report = {'schema': 1, 'passed': False, 'engine_sha256': hashlib.sha256(engine.read_bytes()).hexdigest(),
              'presenter_sha256': hashlib.sha256(presenter.read_bytes()).hexdigest(),
              'scope': '1000 persisted paused tasks; packaged Slint software renderer; continuous redraw diagnostic; real winit redraw timestamps',
              'thresholds': {'frame_p95_ms': 33, 'minimum_frames': 100}}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    ui = None
    try:
        report['populated_core'] = seeded_progress(engine, root)
        with Core(engine, root) as core:
            frames = root/'redraw-times.json'
            env = {**os.environ, 'HLS_V7_DATA_DIR': str(root/'data'),
                   'HLS_V7_DOWNLOAD_DIR': str(root/'downloads'), 'HLS_V7_CORE_TCP': '1',
                   'HLS_V7_CORE_BIND': f'127.0.0.1:{core.port}',
                   'HLS_V7_PIPE': rf'\\.\pipe\HLSNativeFrames-{uuid.uuid4().hex}',
                   'HLS_V7_PRESENTER_FRAME_TRACE': str(frames),
                   'SLINT_DEBUG_PERFORMANCE': 'refresh_full_speed,console'}
            with (root/'renderer.log').open('wb') as log:
                ui = subprocess.Popen([str(presenter), '--workbench'], cwd=presenter.parent,
                                      env=env, stdout=log, stderr=log)
                window = wait(lambda: next(iter(Desktop(backend='uia').windows(process=ui.pid,
                    title_re=r'HLS Downloader [0-9].*', visible_only=True)), None), 30)
                wait(lambda: any(e.element_info.name.startswith('fixture')
                                 for e in window.descendants(control_type='ListItem')), 15)
                user = ctypes.WinDLL('user32', use_last_error=True)
                user.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
                user.GetDpiForWindow.argtypes = [wintypes.HWND]
                user.GetDpiForWindow.restype = wintypes.UINT
                user.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                                             ctypes.c_int, ctypes.c_int, wintypes.UINT]
                user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
                user.SystemParametersInfoW.argtypes = [wintypes.UINT, wintypes.UINT, ctypes.c_void_p, wintypes.UINT]
                report['window_dpi'] = user.GetDpiForWindow(window.handle)
                report['window_title'] = window.window_text()
                time.sleep(args.seconds)
                window.capture_as_image().save(args.report.with_suffix('.png'))
                user.PostMessageW(window.handle, 0x0010, 0, 0)
                wait(frames.is_file, 10)
            values = json.loads(frames.read_text(encoding='utf-8'))
            report['redraw_timestamps_seconds'] = values
            # Remove startup. Keep every interval in the steady measurement window.
            intervals = [(end-start)*1000 for start, end in zip(values, values[1:])
                         if start >= 2]
            assert len(intervals) >= 100, len(intervals)
            report.update(frame_count=len(intervals), measured_seconds=sum(intervals)/1000,
                          frame_p50_ms=percentile(intervals, .5), frame_p95_ms=percentile(intervals, .95),
                          frame_max_ms=max(intervals))
            assert report['frame_p95_ms'] <= 33, report['frame_p95_ms']
            ui.terminate()
            ui.wait(timeout=10)
            env.pop('HLS_V7_PRESENTER_FRAME_TRACE')
            env.pop('SLINT_DEBUG_PERFORMANCE')
            area = wintypes.RECT()
            assert user.SystemParametersInfoW(0x0030, 0, ctypes.byref(area), 0)
            report['layouts'] = []
            report['layout_scope'] = 'Slint scale override at 1/1.25/1.5/2 in the same logical viewport on one hosted display; window may extend beyond hosted display; OS DPI and work area recorded separately; no physical monitor-switch assertion'
            report['hosted_work_area_pixels'] = [area.right-area.left, area.bottom-area.top]
            for scale in (1, 1.25, 1.5, 2):
                env['SLINT_SCALE_FACTOR'] = str(scale)
                with (root/f'layout-{scale}.log').open('wb') as log:
                    ui = subprocess.Popen([str(presenter), '--workbench'], cwd=presenter.parent,
                                          env=env, stdout=log, stderr=log)
                    window = wait(lambda: next(iter(Desktop(backend='uia').windows(process=ui.pid,
                        title_re=r'HLS Downloader [0-9].*', visible_only=True)), None), 30)
                    wait(lambda: window.descendants(control_type='ListItem'), 15)
                    width = round(960*scale)
                    height = round(620*scale)
                    assert user.SetWindowPos(window.handle, None, area.left, area.top, width, height, 0x0004)
                    def resized():
                        outer = wintypes.RECT()
                        assert user.GetWindowRect(window.handle, ctypes.byref(outer))
                        return outer if outer.right-outer.left == width and outer.bottom-outer.top == height else None
                    outer = wait(resized)
                    bounds = outer
                    for name in ('新建', '设置', '工具', '导入', '导出'):
                        def find():
                            return next((e for e in window.descendants(control_type='Button')
                                         if e.element_info.name == name and e.is_visible()), None)
                        button = wait(find, 10)
                        r = button.rectangle()
                        assert bounds.left <= r.left < r.right <= bounds.right and bounds.top <= r.top < r.bottom <= bounds.bottom, (scale, name, bounds, r)
                    window.capture_as_image().save(args.report.with_name(f'scale-{scale}.png'))
                    report['layouts'].append({'slint_scale_factor': scale,
                        'actual_os_window_dpi': user.GetDpiForWindow(window.handle),
                        'window_pixels': [bounds.right-bounds.left, bounds.bottom-bounds.top],
                        'native_outer_pixels': [outer.right-outer.left, outer.bottom-outer.top],
                        'toolbar_accessible_and_inside_window': True})
                    ui.terminate()
                    ui.wait(timeout=10)
            report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc()
    finally:
        if ui is not None and ui.poll() is None:
            ui.terminate()
            ui.wait(timeout=10)
        args.report.write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps({k: v for k, v in report.items() if k != 'redraw_timestamps_seconds'}), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
