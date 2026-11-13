"""Exercise the packaged Slint workbench on a disposable hosted Windows desktop.

UI mutations go through Windows accessibility or real keyboard input. Core IPC
is only the independent observer of their effects, never a substitute UI path.
"""
from __future__ import annotations

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import threading
import time
import traceback
import uuid

from smoke_v7_resource_boundaries import Core, wait
from validation_workspace import test_tmp


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    if os.name != 'nt' or os.environ.get('GITHUB_ACTIONS') != 'true' or not os.environ.get('RUNNER_NAME'):
        raise RuntimeError('Native desktop acceptance is restricted to an isolated GitHub Actions runner')
    from pywinauto import Desktop
    from pywinauto.keyboard import send_keys

    runtime = args.runtime.resolve()
    presenter = runtime/'app/resources/HLSDownloaderPresenter.exe'
    engine = runtime/'app/resources/HLSDownloaderEngine.exe'
    root = test_tmp()/('native-acceptance-' + uuid.uuid4().hex)
    report = {'schema': 1, 'passed': False, 'checks': [], 'scope':
              'Packaged native workbench, real UIA input, isolated Core and loopback HTTP fixture',
              'components': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in (presenter, engine)}}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    ui = None
    payload = b'native workbench download fixture\n'*32768

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_HEAD(self):
            self.send_response(200)
            self.send_header('Content-Length', str(len(payload)))
            self.send_header('Accept-Ranges', 'bytes')
            self.send_header('ETag', '"native-ui-fixture"')
            self.end_headers()

        def do_GET(self):
            start, end = 0, len(payload)-1
            value = self.headers.get('Range')
            if value:
                first, last = value.removeprefix('bytes=').split('-')
                start, end = int(first), int(last) if last else end
            self.send_response(206 if value else 200)
            self.send_header('Content-Length', str(end-start+1))
            self.send_header('ETag', '"native-ui-fixture"')
            if value:
                self.send_header('Content-Range', f'bytes {start}-{end}/{len(payload)}')
            self.end_headers()
            self.wfile.write(payload[start:end+1])

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    desktop = Desktop(backend='uia')

    def save():
        pending = args.report.with_suffix('.pending')
        pending.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
        pending.replace(args.report)

    def tree(window):
        return [{'name': e.element_info.name, 'type': e.element_info.control_type,
                 'enabled': e.is_enabled(), 'visible': e.is_visible(),
                 'bounds': list(e.rectangle())} for e in window.descendants()]

    def window(title):
        def find():
            found = desktop.windows(process=ui.pid, title=title, visible_only=True)
            return found[0] if found else None
        return wait(find, 20)

    def control(window, name, kind=None):
        def find():
            matches = [e for e in window.descendants() if e.element_info.name == name and
                       (kind is None or e.element_info.control_type == kind)]
            return matches[0] if len(matches) == 1 else None
        return wait(find, 10)

    def press(window, name):
        control(window, name, 'Button').invoke()

    try:
        with Core(engine, root) as core:
            core.call({'type': 'store_settings', 'request_id': 11, 'values': {
                'legal_terms_accepted': True, 'legal_terms_version': '2026-08-06-cn-1'}})
            env = {**os.environ, 'HLS_V7_DATA_DIR': str(root/'data'),
                   'HLS_V7_DOWNLOAD_DIR': str(root/'downloads'), 'HLS_V7_CORE_TCP': '1',
                   'HLS_V7_CORE_BIND': f'127.0.0.1:{core.port}',
                   'HLS_V7_PIPE': rf'\\.\pipe\HLSNativeAcceptance-{uuid.uuid4().hex}',
                   'HLS_V7_PRESENTER_TRACE_FILE': str(root/'presenter.log')}
            ui = subprocess.Popen([str(presenter), '--workbench'], cwd=presenter.parent, env=env,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            main_window = wait(lambda: next(iter(desktop.windows(process=ui.pid,
                title_re=r'HLS Downloader [0-9].*', visible_only=True)), None), 25)
            report['initial_tree'] = tree(main_window)
            report['checks'].append({'name': 'accessible_workbench', 'passed': True,
                                     'title': main_window.window_text()})
            save()
            press(main_window, '新建')
            editor = window('新建下载')
            report['editor_tree'] = tree(editor)
            save()
            url = f'http://127.0.0.1:{server.server_port}/native-ui.bin'
            control(editor, '下载地址', 'Edit').set_edit_text(url)
            control(editor, '文件名', 'Edit').set_edit_text('原生界面下载.bin')
            press(editor, '添加任务')
            def completed():
                tasks = core.snapshot()['tasks']
                return next((t for t in tasks if t['url'] == url and t['status'] in ('completed', 'failed')), None)
            task = wait(completed, 30)
            assert task['status'] == 'completed', task
            assert task['filename'] == '原生界面下载.bin', task
            output = Path(task['output_path'])
            assert output.read_bytes() == payload
            report['checks'].append({'name': 'create_download_via_ui', 'passed': True,
                'task_id': task['task_id'], 'sha256': hashlib.sha256(output.read_bytes()).hexdigest()})
            main_window.set_focus()
            send_keys('^a')
            report['selected_tree'] = tree(main_window)
            send_keys('{ESC}')
            press(main_window, '设置')
            settings = window('设置')
            report['settings_tree'] = tree(settings)
            report['checks'].append({'name': 'open_native_settings', 'passed': True})
            save()
            report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc()
        if ui is not None and ui.poll() is None:
            report['failure_windows'] = [{'title': w.window_text(), 'tree': tree(w)}
                for w in desktop.windows(process=ui.pid, visible_only=True)]
    finally:
        if ui is not None and ui.poll() is None:
            ui.terminate()
            ui.wait(timeout=10)
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        save()
    print(json.dumps({'passed': report['passed'], 'checks': report['checks'], 'error': report.get('error')},
                     ensure_ascii=False), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
