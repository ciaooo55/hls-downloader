"""Exercise the packaged Slint workbench on a disposable hosted Windows desktop.

UI mutations go through Windows accessibility or real keyboard input. Core IPC
is only the independent observer of their effects, never a substitute UI path.
"""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import subprocess
import threading
import time
import traceback
import uuid
import zipfile

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
    from pywinauto import mouse
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
    archive = io.BytesIO()
    with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as output:
        output.writestr('docs/readme.txt', 'Native ZIP fixture')
        output.writestr('media/notes.txt', 'Archive metadata fixture')
    files = {'/bundle.zip': (archive.getvalue(), 'application/zip'),
             '/index.html': (b'<a href="/crawl.bin">crawl.bin</a>', 'text/html')}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_HEAD(self):
            self.reply(False)

        def do_GET(self):
            self.reply(True)

        def reply(self, body):
            data, content_type = files.get(self.path, (payload, 'application/octet-stream'))
            start, end = 0, len(data)-1
            value = self.headers.get('Range')
            if value:
                first, last = value.removeprefix('bytes=').split('-')
                if first:
                    start, end = int(first), min(int(last), end) if last else end
                else:
                    start = max(0, len(data)-int(last))
            self.send_response(206 if value else 200)
            self.send_header('Content-Length', str(end-start+1))
            self.send_header('Content-Type', content_type)
            self.send_header('Accept-Ranges', 'bytes')
            self.send_header('ETag', '"native-ui-fixture"')
            if value:
                self.send_header('Content-Range', f'bytes {start}-{end}/{len(data)}')
            self.end_headers()
            if body:
                self.wfile.write(data[start:end+1])

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
        def bounds(element):
            r = element.rectangle()
            return [r.left, r.top, r.right, r.bottom]
        return [{'name': e.element_info.name, 'type': e.element_info.control_type,
                 'enabled': e.is_enabled(), 'visible': e.is_visible(),
                 'bounds': bounds(e)} for e in window.descendants()]

    def window(title):
        def find():
            found = desktop.windows(process=ui.pid, title=title, visible_only=True)
            if not found:
                found = [e for top in desktop.windows(process=ui.pid, visible_only=True)
                         for e in top.descendants(control_type='Window') if e.element_info.name == title]
            return found[0] if found else None
        return wait(find, 20)

    def control(window, name, kind=None):
        def find():
            matches = [e for e in window.descendants() if e.element_info.name == name and
                       (kind is None or e.element_info.control_type == kind)]
            return matches[0] if len(matches) == 1 else None
        return wait(find, 10)

    def press(window, name):
        button = control(window, name, 'Button')
        wait(button.is_enabled, 10)
        button.invoke()

    def type_field(window, name, text):
        field = control(window, name, 'Edit')
        rect = field.rectangle()
        mouse.click(coords=((rect.left+rect.right)//2, (rect.top+rect.bottom)//2))
        send_keys('^a')
        for index, line in enumerate(text.split('\n')):
            if index:
                send_keys('{ENTER}')
            send_keys(line, with_spaces=True, vk_packet=True)
        send_keys('{TAB}')

    def choose_file(path):
        dialog = wait(lambda: next(iter(desktop.windows(process=ui.pid,
            class_name='#32770', visible_only=True)), None), 15)
        report.setdefault('file_dialogs', []).append(tree(dialog))
        dialog.set_focus()
        # The hosted Windows common file picker exposes Alt+N for File name.
        send_keys('%n')
        send_keys('^a')
        send_keys(str(path), with_spaces=True, vk_packet=True)
        send_keys('{ENTER}')
        wait(lambda: not dialog.is_visible(), 15)

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
            press(main_window, '设置')
            settings = window('设置')
            report['settings_tree'] = tree(settings)
            press(settings, '取消')
            press(main_window, '新建')
            editor = window('新建下载')
            report['editor_tree'] = tree(editor)
            save()
            url = f'http://127.0.0.1:{server.server_port}/native-ui.bin'
            control(editor, '下载地址', 'Edit').set_edit_text(url)
            control(editor, '文件名', 'Edit').set_edit_text('原生界面下载.bin')
            report['editor_values'] = {name: control(editor, name, 'Edit').get_value()
                                       for name in ('下载地址', '文件名', '保存位置')}
            save()
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
            rows = wait(lambda: [e for e in main_window.descendants(control_type='ListItem')
                                 if e.element_info.name.startswith(task['filename'])])
            rows[0].click_input()
            send_keys('^a')
            wait(lambda: any(e.element_info.name == '已选 1 项' for e in main_window.descendants()))
            report['selected_tree'] = tree(main_window)
            send_keys('{ESC}')
            wait(lambda: not any(e.element_info.name == '已选 1 项' for e in main_window.descendants()))
            report['checks'].append({'name': 'keyboard_select_all_and_escape', 'passed': True})
            rows[0].click_input()
            send_keys('{ENTER}')
            details = window('任务详情与日志')
            assert control(details, '下载地址', 'Edit').get_value() == url
            report['details_tree'] = tree(details)
            press(details, '关闭')
            report['checks'].append({'name': 'keyboard_task_details', 'passed': True})
            press(main_window, '设置')
            settings = window('设置')
            report['settings_tree'] = tree(settings)
            control(settings, '下载', 'TabItem').invoke()
            type_field(settings, '默认并发连接数', '3')
            press(settings, '保存设置')
            wait(lambda: core.call({'type': 'load_settings', 'request_id': 12})['default_concurrency'] == 3)
            press(settings, '取消')
            report['checks'].append({'name': 'save_native_settings_persists_to_core', 'passed': True})
            press(main_window, '管理队列')
            queues = window('下载队列')
            press(queues, '新建队列')
            control(queues, '队列名称', 'Edit').set_edit_text('原生验收队列')
            press(queues, '保存队列')
            profiles = wait(lambda: [p for p in core.call({'type': 'load_settings', 'request_id': 13})['queue_profiles']
                                    if p['name'] == '原生验收队列'])
            report['queue_profile'] = profiles[0]
            press(queues, '取消')
            report['checks'].append({'name': 'create_named_queue_via_ui', 'passed': True})
            press(main_window, '工具')
            press(main_window, 'ZIP 内容预览')
            resources = window('ZIP 内容预览')
            control(resources, '资源来源', 'Edit').set_edit_text(f'http://127.0.0.1:{server.server_port}/bundle.zip')
            press(resources, '读取 / 发现')
            wait(lambda: any(e.element_info.name == 'docs/readme.txt' for e in resources.descendants()))
            report['zip_tree'] = tree(resources)
            press(resources, '关闭')
            report['checks'].append({'name': 'remote_zip_preview_via_ui', 'passed': True})
            press(main_window, '工具')
            press(main_window, '抓站与同步项目')
            crawl = window('抓站与同步项目')
            control(crawl, '资源来源', 'Edit').set_edit_text(f'http://127.0.0.1:{server.server_port}/index.html')
            press(crawl, '读取 / 发现')
            wait(lambda: any('crawl.bin' in e.element_info.name for e in crawl.descendants()), 30)
            press(crawl, '全选')
            press(crawl, '刷新')
            wait(lambda: control(crawl, '下载所选', 'Button').is_enabled())
            checkboxes = [e for e in crawl.descendants(control_type='CheckBox')
                          if '/crawl.bin' in e.element_info.name]
            assert len(checkboxes) == 1 and checkboxes[0].get_toggle_state() == 1, tree(crawl)
            press(crawl, '下载所选')
            wait(lambda: any(t['url'].endswith('/crawl.bin') and t['status'] == 'completed'
                             for t in core.snapshot()['tasks']), 30)
            press(crawl, '关闭')
            report['checks'].append({'name': 'crawl_create_discover_enqueue_via_ui', 'passed': True})
            press(main_window, '工具')
            press(main_window, '批量添加')
            batch = window('批量添加')
            type_field(batch, '下载地址', '\n'.join(
                f'http://127.0.0.1:{server.server_port}/batch-{n}.bin' for n in (1, 2)))
            report['batch_input'] = control(batch, '下载地址', 'Edit').get_value()
            save()
            press(batch, '添加任务')
            batch_tasks = wait(lambda: [t for t in core.snapshot()['tasks']
                if '/batch-' in t['url'] and t['status'] == 'completed'] or None, 30)
            wait(lambda: len([t for t in core.snapshot()['tasks']
                             if '/batch-' in t['url'] and t['status'] == 'completed']) == 2)
            report['checks'].append({'name': 'batch_create_via_ui', 'passed': True})
            main_window.set_focus()
            rows = sorted(main_window.descendants(control_type='ListItem'), key=lambda r: r.rectangle().top)
            assert len(rows) == 4, tree(main_window)
            rows[0].click_input()
            send_keys('+{DOWN}')
            wait(lambda: any(e.element_info.name == '已选 2 项' for e in main_window.descendants()))
            send_keys('^a')
            wait(lambda: any(e.element_info.name == '已选 4 项' for e in main_window.descendants()))
            send_keys('{ESC}')
            report['checks'].append({'name': 'keyboard_shift_range_and_multi_select', 'passed': True})
            press(main_window, '工具')
            press(main_window, '队列顺序')
            before = sorted(core.snapshot()['tasks'], key=lambda t: (t['queue_index'], t['task_id']))
            handles = sorted([e for e in main_window.descendants(control_type='Button')
                              if e.element_info.name == '调整任务队列顺序'], key=lambda e: e.rectangle().top)
            assert len(handles) == 4, tree(main_window)
            r = handles[0].rectangle()
            start = ((r.left+r.right)//2, (r.top+r.bottom)//2)
            mouse.press(coords=start)
            mouse.move(coords=(start[0], start[1]+64))
            mouse.release(coords=(start[0], start[1]+64))
            expected = [before[1]['task_id'], before[0]['task_id'], before[2]['task_id'], before[3]['task_id']]
            wait(lambda: [t['task_id'] for t in sorted(core.snapshot()['tasks'],
                key=lambda t: (t['queue_index'], t['task_id']))] == expected)
            report['checks'].append({'name': 'drag_queue_order_via_ui', 'passed': True, 'order': expected})
            export = root/'任务 导出.json'
            control(main_window, '导出', 'Button').click_input()
            choose_file(export)
            wait(export.is_file)
            assert url in export.read_text(encoding='utf-8-sig')
            links = root/'导入 链接.txt'
            links.write_text(f'http://127.0.0.1:{server.server_port}/imported.bin\n', encoding='utf-8')
            control(main_window, '导入', 'Button').click_input()
            choose_file(links)
            imported = wait(lambda: next((t for t in core.snapshot()['tasks']
                if t['url'].endswith('/imported.bin') and t['status'] == 'completed'), None), 30)
            assert Path(imported['output_path']).read_bytes() == payload
            report['checks'].append({'name': 'native_file_picker_import_export', 'passed': True})
            user = ctypes.WinDLL('user32', use_last_error=True)
            user.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                                         ctypes.c_int, ctypes.c_int, wintypes.UINT]
            user.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
            report['layouts'] = []
            for width, height in ((620, 480), (960, 640), (1400, 820)):
                assert user.SetWindowPos(main_window.handle, None, 0, 0, width, height, 0x0004)
                def resized():
                    outer = wintypes.RECT()
                    assert user.GetWindowRect(main_window.handle, ctypes.byref(outer))
                    return outer if outer.right-outer.left == width and outer.bottom-outer.top == height else None
                outer = wait(resized)
                bounds = outer
                for name in ('新建', '设置', '工具', '导入', '导出'):
                    button = control(main_window, name, 'Button')
                    r = button.rectangle()
                    assert button.is_visible() and bounds.left <= r.left < r.right <= bounds.right and bounds.top <= r.top < r.bottom <= bounds.bottom, (name, bounds, r)
                main_window.capture_as_image().save(args.report.with_name(f'layout-{width}x{height}.png'))
                report['layouts'].append({'width': bounds.right-bounds.left, 'height': bounds.bottom-bounds.top,
                                          'native_outer_pixels': [outer.right-outer.left, outer.bottom-outer.top],
                                          'toolbar_accessible_and_inside_window': True})
            report['checks'].append({'name': 'responsive_native_window_sizes', 'passed': True,
                'scope': 'actual window resizing on hosted Windows; not physical multi-monitor DPI validation'})
            main_window.capture_as_image().save(args.report.with_suffix('.png'))
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
