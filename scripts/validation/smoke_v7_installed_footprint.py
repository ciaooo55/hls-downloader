"""Sandbox-only install at a custom path, actual UI startup, resource usage and uninstall."""
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
import shutil
import socket
import subprocess
import threading
import time
import traceback
from urllib.request import Request, urlopen
import uuid
import zipfile

import psutil
from PIL import Image

from msi_checkpoint_fixture import connect, send, wait_task
from smoke_v7_download_playback import wait


def downloads_folder():
    class Guid(ctypes.Structure):
        _fields_ = [('a', ctypes.c_uint32), ('b', ctypes.c_uint16), ('c', ctypes.c_uint16),
                    ('d', ctypes.c_ubyte*8)]
    folder = Guid.from_buffer_copy(uuid.UUID('374de290-123f-4565-9164-39c4925e467b').bytes_le)
    path = ctypes.c_wchar_p()
    shell = ctypes.windll.shell32
    shell.SHGetKnownFolderPath.argtypes = [ctypes.POINTER(Guid), wintypes.DWORD,
                                         wintypes.HANDLE, ctypes.POINTER(ctypes.c_wchar_p)]
    assert shell.SHGetKnownFolderPath(ctypes.byref(folder), 0, None, ctypes.byref(path)) == 0
    result = Path(path.value)
    ctypes.windll.ole32.CoTaskMemFree.argtypes = [ctypes.c_void_p]
    ctypes.windll.ole32.CoTaskMemFree(ctypes.cast(path, ctypes.c_void_p))
    return result


def free_port():
    with socket.socket() as server:
        server.bind(('127.0.0.1', 0))
        return server.getsockname()[1]


def verify_http_retry(ipc, download_dir):
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            status = int(self.path.strip('/').split('-')[0])
            self.send_response(206 if status == 200 else status)
            if status in (200, 206):
                self.send_header('Content-Range', 'bytes 0-0/1')
            if status == 206:
                self.send_header('ETag', '"changed-entity"')
            self.send_header('Content-Length', '1' if status in (200, 206) else '0')
            self.send_header('Connection', 'close')
            self.end_headers()
            if status in (200, 206):
                self.wfile.write(b'x')

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    cases = []
    try:
        send(ipc, {'type': 'store_settings', 'request_id': 100, 'values': {
            'auto_retry_failed_max': 2, 'queue_max_active': 1}})
        for status, attempts in ((403, 1), (503, 3), (206, 1)):
            ids = []
            for code in (status, 200):
                created = send(ipc, {'type': 'command', 'request_id': 101, 'command': {
                    'kind': 'create_task', 'spec': {
                        'url': f'http://127.0.0.1:{server.server_port}/{code}-{status}.bin',
                        'filename': f'retry-{code}-{status}.bin', 'resource_kind': 'file',
                        'expected_size': 1, 'etag': '"retry-fixture"', 'concurrency': 1,
                        'proxy': 'hls-downloader://direct-proxy'}}})
                ids.append(next(e['event']['snapshot']['task_id'] for e in created['events']
                                if e['event']['kind'] == 'task_created'))
            completed = wait_task(ipc, ids[1], lambda t: t['status'] in ('completed', 'failed'), 102)
            assert completed['status'] == 'completed', completed
            assert Path(completed['output_path']).read_bytes() == b'x'
            snapshots = send(ipc, {'type': 'snapshot', 'request_id': 103})['tasks']
            failed = next(task for task in snapshots if task['task_id'] == ids[0])
            assert failed['status'] == 'failed', failed
            if status == 206:
                assert failed['error_code'] == 'RESOURCE_CHANGED', failed
                assert '新建任务' in failed['error_hint'], failed
                assert not (download_dir/f'retry-{status}-{status}.bin').exists()
            else:
                assert failed['http_status'] == status, failed
            assert failed['error_attempt'] == attempts and 'retry' in failed['available_actions'], failed
            for tid in ids:
                send(ipc, {'type': 'command', 'request_id': 104, 'command': {
                    'kind': 'task_action', 'task_id': tid, 'action': 'delete_files'}})
                assert not (download_dir/'.hls-tasks'/tid).exists(), tid
            assert not Path(completed['output_path']).exists()
            cases.append({'http_status': status, 'task_attempts': attempts,
                          'following_download_verified': True, 'test_tasks_and_files_removed': True})
    finally:
        send(ipc, {'type': 'store_settings', 'request_id': 105, 'values': {
            'auto_retry_failed_max': 0, 'queue_max_active': 3}})
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    return cases


def verify_hls_retry(ipc, download_dir, media_bytes):
    counts = {}
    counts_lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            with counts_lock:
                hit = counts[self.path] = counts.get(self.path, 0) + 1
            route = self.path.strip('/')
            if route.endswith('.m3u8'):
                case = route.removesuffix('.m3u8')
                byterange = f'#EXT-X-BYTERANGE:{len(media_bytes)}@0\n' if case.startswith('range-') else ''
                body = f'#EXTM3U\n#EXTINF:1,\n{byterange}{case}.ts\n#EXT-X-ENDLIST\n'.encode()
                status = 200
            else:
                case = route.removesuffix('.ts')
                ranged = case.startswith('range-')
                case = case.removeprefix('range-')
                status = (403 if case == 'forbidden' else
                          503 if case == 'transient' and hit < 3 else
                          206 if ranged else 200)
                body = media_bytes if status in (200, 206) else b''
                if ranged:
                    assert self.headers.get('Range') == f'bytes=0-{len(media_bytes)-1}'
            self.send_response(status)
            self.send_header('Content-Length', str(len(body)))
            if status == 206:
                self.send_header('Content-Range', f'bytes 0-{len(body)-1}/{len(body)}')
            self.send_header('Connection', 'close')
            self.end_headers()
            if not route.endswith('.m3u8') and case == 'truncated' and hit == 1:
                body = body[:len(body)//2]
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    cases = []
    try:
        send(ipc, {'type': 'store_settings', 'request_id': 110, 'values': {
            'auto_retry_failed_max': 0, 'queue_max_active': 1}})
        for case in ('forbidden', 'range-forbidden', 'transient', 'range-transient',
                     'truncated', 'range-truncated'):
            created = send(ipc, {'type': 'command', 'request_id': 111, 'command': {
                'kind': 'create_task', 'spec': {
                    'url': f'http://127.0.0.1:{server.server_port}/{case}.m3u8',
                    'filename': f'hls-retry-{case}.webm', 'resource_kind': 'hls',
                    'concurrency': 1, 'proxy': 'hls-downloader://direct-proxy'}}})
            tid = next(e['event']['snapshot']['task_id'] for e in created['events']
                       if e['event']['kind'] == 'task_created')
            task = wait_task(ipc, tid, lambda t: t['status'] in ('completed', 'failed'), 112)
            # 安装包含 curl：HTTP 403 会先经过既有的单次浏览器身份回退。
            expected_requests = 2 if case.endswith('forbidden') else 3 if case.endswith('transient') else 2
            with counts_lock:
                requests = counts.get(f'/{case}.ts', 0)
            assert requests == expected_requests, (case, requests, task)
            if case.endswith('forbidden'):
                assert task['status'] == 'failed' and task['http_status'] == 403, task
            else:
                assert task['status'] == 'completed', task
                assert Path(task['output_path']).read_bytes() == media_bytes, case
                wait(lambda: not (download_dir/'.hls-tasks'/tid).exists(), f'{case}: completed cache remains')
            send(ipc, {'type': 'command', 'request_id': 113, 'command': {
                'kind': 'task_action', 'task_id': tid, 'action': 'delete_files'}})
            assert not (download_dir/'.hls-tasks'/tid).exists(), case
            assert not Path(task['output_path']).is_file(), case
            cases.append({'case': case, 'segment_requests': requests, 'status': task['status'],
                          'test_tasks_and_files_removed': True})
    finally:
        send(ipc, {'type': 'store_settings', 'request_id': 114, 'values': {'queue_max_active': 3}})
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
    return cases


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    args.report.parent.mkdir(parents=True, exist_ok=True)
    product = json.loads(args.manifest.read_text(encoding='utf-8-sig'))['product_code']
    install = Path(r'C:\QAWork\Custom Install Path') / ('HLSDownloader-' + uuid.uuid4().hex[:8])
    assert not install.exists(), 'Fresh custom installation required'
    report = {'passed': False, 'checks': [], 'msi_sha256': hashlib.sha256(args.msi.read_bytes()).hexdigest()}
    processes = []
    installed = False
    stage = Path(r'C:\QAWork') / ('installer-' + product.strip('{}'))
    stage.mkdir(parents=True, exist_ok=True)
    local_msi = stage/'package.msi'
    # Windows Installer 服务不共享桌面用户的沙盒映射，安装文件和日志先落在虚拟机本地。
    shutil.copyfile(args.msi, local_msi)
    assert hashlib.sha256(local_msi.read_bytes()).hexdigest() == report['msi_sha256']
    query = ctypes.windll.msi.MsiQueryProductStateW
    query.argtypes = [wintypes.LPCWSTR]
    query.restype = ctypes.c_int

    def record(name, actual=True):
        report['checks'].append({'name': name, 'actual': actual})
        save()
        print(name, flush=True)

    def save():
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    def terminate():
        all_processes = []
        for proc in psutil.process_iter(['exe']):
            if proc.info['exe'] and Path(proc.info['exe']).is_relative_to(install):
                all_processes.append(proc)
        for proc in all_processes:
            try:
                proc.terminate()
            except psutil.NoSuchProcess:
                pass
        psutil.wait_procs(all_processes, timeout=15)

    try:
        record('MSI staged on VM local disk with matching SHA256')
        # msiexec 的属性必须只引号包围值，list2cmdline 会错误地引号包围整个 KEY=value。
        command_line = subprocess.list2cmdline(['msiexec.exe', '/i', str(local_msi), '/qn',
            '/norestart', '/l*v', str(stage/'install.log')]) + f' INSTALLDIR="{install}"'
        code = subprocess.run(command_line, timeout=180).returncode
        assert code in (0, 3010), f'MSI install exit {code}'
        installed = True
        assert query(product) == 5 and (install/'HLSDownloader.exe').is_file()
        record('MSI installs at requested path containing spaces', str(install))
        resources = install/'app/resources'
        paths = [path for path in install.rglob('*') if path.is_file()]
        report['installed_bytes'] = sum(path.stat().st_size for path in paths)
        report['installed_files'] = len(paths)
        report['largest_files'] = [{'path': p.relative_to(install).as_posix(), 'bytes': p.stat().st_size}
                                  for p in sorted(paths, key=lambda p: p.stat().st_size, reverse=True)[:12]]
        report['installed_components_sha256'] = {name: hashlib.sha256((resources/name).read_bytes()).hexdigest()
            for name in ('HLSDownloaderEngine.exe', 'HLSDownloaderNativeHost.exe',
                         'HLSDownloaderPresenter.exe', 'HLSDownloaderUpdater.exe')}
        if args.expected_components:
            expected_components = json.loads(args.expected_components.read_text(encoding='utf-8-sig'))
            actual_components = {}
            for relative, expected_hash in expected_components.items():
                path = install / relative
                assert path.resolve().is_relative_to(install.resolve()), relative
                actual_components[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
                assert actual_components[relative] == expected_hash, f'Installed payload differs: {relative}'
            record('installed payload hashes match the tested components and delivery archives', actual_components)
        version = json.loads((resources/'BUILD-PROVENANCE.json').read_text(encoding='utf-8-sig'))['product_version']
        extensions = {}
        for family in ('Chromium', 'Firefox'):
            root = install/'extensions'/family
            archives = list((resources/'extensions').glob(f'*-{family}.zip'))
            assert archives, f'Installed {family} archive missing'
            for archive in archives:
                with zipfile.ZipFile(archive) as zipped:
                    files = {name for name in zipped.namelist() if not name.endswith('/')}
                    assert zipped.testzip() is None, f'Corrupt installed extension: {family}'
                    manifest = json.loads(zipped.read('manifest.json'))
                    assert manifest['version'] == version and manifest['action']['default_popup'] == 'popup.html'
                    assert {'background.js', 'popup.html'} <= files
                    # 当前安装包按需解压插件；旧安装映像若附带解压目录，仍核对两份内容。
                    if root.exists():
                        assert files == {p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_file()}
                        for name in files:
                            assert (root/name).read_bytes() == zipped.read(name), f'Installed extension mismatch: {family}/{name}'
                    background_sha256 = hashlib.sha256(zipped.read('background.js')).hexdigest()
            extensions[family] = {'background_sha256': background_sha256,
                'unpacked_directory_present': root.exists(),
                'archives_sha256': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in archives}}
        record('installed extension ZIPs are valid; unpacked copies match when present', extensions)
        port, ui_port = free_port(), free_port()
        token = uuid.uuid4().hex
        # 安装验收必须加载真实任务库，不能继承前一轮离线界面夹具。
        env = {**{key: value for key, value in os.environ.items() if not key.startswith('HLS_UI_AUDIT_')},
               'HLS_V7_CORE_TCP': '1', 'HLS_V7_CORE_BIND': f'127.0.0.1:{port}',
               'HLS_V7_STARTUP_PROFILE': '1',
               'HLS_UI_TEST_API': '1', 'HLS_UI_TEST_PORT': str(ui_port), 'HLS_UI_TEST_TOKEN': token}
        for key in ('HLS_V7_DATA_DIR', 'HLS_V7_DOWNLOAD_DIR', 'HLS_V7_PIPE', 'HLS_ENGINE_PATH'):
            env.pop(key, None)
        log = args.report.with_suffix('.runtime.log').open('wb')
        processes.append(subprocess.Popen([str(resources/'HLSDownloaderEngine.exe')], env=env,
            stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW))
        with connect(port) as ipc:
            settings = send(ipc, {'type': 'load_settings', 'request_id': 1})
            expected = downloads_folder()
            assert Path(settings['download_dir']) == expected, settings['download_dir']
            record('default download directory matches Windows Known Folder', str(expected))
            send(ipc, {'type': 'store_settings', 'request_id': 2, 'values': {
                'legal_terms_accepted': True, 'legal_terms_version': '2026-08-06-cn-1',
                'av_scan_enabled': False, 'download_progress_window_enabled': False,
                'download_complete_popup_enabled': False}})
            url = 'https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm'
            with urlopen(url, timeout=45) as response:
                expected_body = response.read(8*1024*1024+1)
            assert 0 < len(expected_body) <= 8*1024*1024
            created = send(ipc, {'type': 'command', 'request_id': 3, 'command': {
                'kind': 'create_task', 'spec': {'url': url, 'resource_kind': 'file',
                    'filename': 'hls-acceptance-public.webm', 'concurrency': 2,
                    'proxy': 'hls-downloader://direct-proxy'}}})
            tid = next(e['event']['snapshot']['task_id'] for e in created['events']
                       if e['event']['kind'] == 'task_created')
            downloaded = wait_task(ipc, tid, lambda t: t['status'] in ('completed', 'failed'), 90)
            assert downloaded['status'] == 'completed', downloaded
            output = Path(downloaded['output_path'])
            assert output.parent == expected and output.read_bytes() == expected_body
            wait(lambda: not (expected/'.hls-tasks'/tid).exists(), 'Public download cache remains')
            record('real public MDN video downloads into the default folder with identical bytes',
                   {'url': url, 'bytes': len(expected_body),
                    'sha256': hashlib.sha256(expected_body).hexdigest()})
            deleted = send(ipc, {'type': 'command', 'request_id': 4, 'command': {
                'kind': 'task_action', 'task_id': tid, 'action': 'delete_files'}})
            assert 'events' in deleted and not output.exists(), deleted
            if args.verify_http_retry:
                record('installed release Engine respects HTTP retry policy and removes test caches',
                       verify_http_retry(ipc, expected))
            if args.verify_hls_retry:
                record('installed release Engine retries recoverable HLS segments and cleans completed/deleted caches',
                       verify_hls_retry(ipc, expected, expected_body))
        report['ui_exercised'] = not args.core_only
        if not args.core_only:
            started = time.monotonic()
            processes.append(subprocess.Popen([str(install/'HLSDownloader.exe')], cwd=install,
                                              env=env, stdout=log, stderr=log))

            def ui(path, raw=False):
                with urlopen(Request(f'http://127.0.0.1:{ui_port}/{path}',
                            headers={'X-HLS-Test-Token': token}), timeout=5) as response:
                    body = response.read()
                    return body if raw else json.loads(body)

            def ready():
                try:
                    window, state = ui('window'), ui('state')
                    return window if window.get('showing') and '已连接' in state.get('engineText', '') else None
                except OSError:
                    return None
            window = wait(ready, 'Installed UI not visibly connected', 120)
            report['ui_visible_connected_seconds'] = time.monotonic()-started
            screen = ui('screenshot', True)
            paint = ui('screenshot?mode=paint', True)
            args.report.with_suffix('.png').write_bytes(screen)
            args.report.with_suffix('.paint.png').write_bytes(paint)
            def nonblank(data):
                with Image.open(io.BytesIO(data)) as image:
                    # Resampling a flat canvas's rounded corners creates many
                    # colors without proving that any application content drew.
                    rgb = image.convert('RGB')
                    pixels = rgb.width * rgb.height
                    colors = rgb.getcolors(pixels)
                    counts = sorted((count for count, _ in colors), reverse=True)
                    return pixels - sum(counts[:2]) > max(500, pixels * .005)
            report['ui_capture'] = {'screen_nonblank': nonblank(screen), 'paint_nonblank': nonblank(paint)}
            assert any(report['ui_capture'].values()), 'UI reports showing but both screen and paint captures are blank'
            record('installed UI reports showing, connects to Core and produces a nonblank render',
                   {'window': window, 'capture': report['ui_capture']})
            if args.verify_browser_color:
                # 比较同一页脚在握手前后的像素；引擎已有的绿色不计入变化。
                endpoint = 'screenshot' if report['ui_capture']['screen_nonblank'] else 'screenshot?mode=paint'

                def footer_green(data):
                    with Image.open(io.BytesIO(data)) as image:
                        rgb = image.convert('RGB')
                        footer = rgb.crop((rgb.width * 2 // 3, max(0, rgb.height - 50), rgb.width, rgb.height))
                        return sum(count for count, (r, g, b) in footer.getcolors(footer.width * footer.height)
                                   if g > r * 1.25 and g > b * 1.15)

                before = ui(endpoint, True)
                before_green = footer_green(before)
                with connect(port) as ipc:
                    reply = send(ipc, {'type': 'command', 'request_id': 20, 'command': {
                        'kind': 'browser_hello', 'version': version, 'browser': 'sandbox-verification'}})
                    assert 'events' in reply, reply

                def connected_footer():
                    data = ui(endpoint, True)
                    return data if footer_green(data) - before_green > 30 else None

                connected = wait(connected_footer, 'Browser handshake did not turn the footer green', 15)
                args.report.with_suffix('.disconnected.png').write_bytes(before)
                args.report.with_suffix('.connected.png').write_bytes(connected)
                record('simulated browser handshake changes the rendered connection label to green',
                       {'before_green_pixels': before_green, 'after_green_pixels': footer_green(connected),
                        'transport': 'Core IPC browser_hello; no real browser extension used'})
            time.sleep(3)


        def sample():
            result = {}
            for proc in psutil.process_iter(['exe', 'name']):
                if proc.info['exe'] and Path(proc.info['exe']).is_relative_to(install):
                    cpu, memory = proc.cpu_times(), proc.memory_info()
                    result[proc.pid] = {'name': proc.info['name'], 'cpu': cpu.user+cpu.system,
                                       'rss_bytes': memory.rss, 'private_bytes': getattr(memory, 'private', 0)}
            return result
        before = sample()
        started = time.monotonic()
        time.sleep(5)
        after = sample()
        elapsed = time.monotonic()-started
        cpu = sum(max(0, p['cpu']-before.get(pid, p)['cpu']) for pid, p in after.items())
        report['idle'] = {'seconds': elapsed, 'logical_cpus': psutil.cpu_count(),
            'cpu_one_core_percent': cpu/elapsed*100,
            'cpu_machine_percent': cpu/elapsed*100/psutil.cpu_count(), 'processes': after,
            'rss_sum_bytes': sum(p['rss_bytes'] for p in after.values()),
            'private_sum_bytes': sum(p['private_bytes'] for p in after.values())}
        record('measured installed runtime idle CPU and memory', report['idle'])
        terminate()
        code = subprocess.run(['msiexec.exe', '/x', product, '/qn', '/norestart', '/l*v',
                str(stage/'uninstall.log')], timeout=180).returncode
        assert code in (0, 3010) and query(product) == -1, f'Uninstall failed: {code}'
        installed = False
        assert not (install/'HLSDownloader.exe').exists()
        record('MSI uninstalls and removes its product registration')
        report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc()
        report['launched_processes'] = [{'pid': proc.pid, 'exit_code': proc.poll()}
                                        for proc in processes]
    finally:
        terminate()
        if installed:
            report['cleanup_uninstall_exit'] = subprocess.run(['msiexec.exe', '/x', product,
                '/qn', '/norestart'], timeout=180).returncode
        for name in ('install', 'uninstall'):
            log_path = stage/(name+'.log')
            if log_path.exists():
                shutil.copyfile(log_path, args.report.with_suffix('.'+name+'.log'))
        save()
        print(json.dumps(report, ensure_ascii=False), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--msi', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--expected-components', type=Path,
                        help='JSON mapping installation-relative paths to tested SHA256 values')
    parser.add_argument('--core-only', action='store_true', help='Skip UI when desktop interaction is blocked')
    parser.add_argument('--verify-browser-color', action='store_true', help='Verify footer pixels before/after a simulated browser handshake')
    parser.add_argument('--verify-http-retry', action='store_true', help='Verify permanent/transient HTTP failures and queue continuation with the installed Engine')
    parser.add_argument('--verify-hls-retry', action='store_true', help='Verify full/range HLS segment errors, recovery and cache cleanup with the installed Engine')
    raise SystemExit(run(parser.parse_args()))
