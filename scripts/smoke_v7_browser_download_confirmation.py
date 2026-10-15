"""Public Firefox media -> actual desktop confirmation -> verified file."""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile
import time
import uuid
import zipfile
import subprocess

import psutil

from selenium import webdriver
from selenium.webdriver import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service
from selenium.webdriver.edge.options import Options as EdgeOptions
from selenium.webdriver.edge.service import Service as EdgeService

from smoke_extension_media import _find_firefox, _find_edge, _fit_browser_window
from smoke_v7_portable_app import Pipe, wait, runtime_processes
from smoke_v7_presenter_visual import capture, find_window, print_window, user32
from PIL import ImageGrab

PAGE = 'https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html'
MEDIA = 'https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm'
SHA256 = 'c6f8a348953395598a9a73b9bab1676436410797bce9f398f4be1531d6e76dda'
os.environ.setdefault('HLS_V7_PRESENTER_TRACE_FILE', str(Path(tempfile.gettempdir()) / 'hls-downloader-presenter-trace.log'))
_last_presenter_diagnostic = 0.0
_presenter_self_test_done = False


def presenter_window(title='确认下载', runtime=None):
    global _last_presenter_diagnostic, _presenter_self_test_done
    runtime = runtime or Path(__file__).resolve().parents[1] / '.tool-cache/runtime/HLSDownloader'
    runtime = runtime.resolve()
    processes = runtime_processes(runtime)
    presenters = [process for process in processes if process['name'] == 'HLSDownloaderPresenter.exe']
    for process in presenters:
        handle = find_window(process['pid'], title)
        if handle:
            return handle

    now = time.monotonic()
    if now - _last_presenter_diagnostic >= 10:
        startup_diagnostic = None
        if not presenters and not _presenter_self_test_done:
            _presenter_self_test_done = True
            # self-test 只创建并隐藏独立测试窗，不取单实例锁、不连接 Core、不处理下载请求。
            try:
                probe = subprocess.run([str(runtime / 'app/resources/HLSDownloaderPresenter.exe'), '--self-test'],
                    cwd=runtime / 'app/resources', capture_output=True, timeout=10)
                startup_diagnostic = {'exit_code': probe.returncode,
                    'stdout': probe.stdout.decode('utf-8', errors='replace')[-4000:],
                    'stderr': probe.stderr.decode('utf-8', errors='replace')[-4000:]}
            except subprocess.TimeoutExpired:
                startup_diagnostic = {'error': 'Presenter self-test timed out'}
        windows = []
        user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
        for process in presenters:
            pid = process['pid']

            @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
            def visit(hwnd, _lparam):
                owner = wintypes.DWORD()
                user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
                if owner.value != pid:
                    return True
                length = user32.GetWindowTextLengthW(hwnd)
                title_buffer = ctypes.create_unicode_buffer(length + 1)
                class_buffer = ctypes.create_unicode_buffer(256)
                rect = wintypes.RECT()
                user32.GetWindowTextW(hwnd, title_buffer, length + 1)
                user32.GetClassNameW(hwnd, class_buffer, len(class_buffer))
                user32.GetWindowRect(hwnd, ctypes.byref(rect))
                windows.append({
                    'pid': pid, 'hwnd': int(hwnd), 'visible': bool(user32.IsWindowVisible(hwnd)),
                    'title': title_buffer.value, 'class': class_buffer.value,
                    'bounds': [rect.left, rect.top, rect.right, rect.bottom],
                })
                return True

            user32.EnumWindows(visit, 0)

        all_presenter_processes = []
        for process in psutil.process_iter(['pid', 'name', 'exe']):
            try:
                if 'presenter' in (process.info.get('name') or '').casefold():
                    all_presenter_processes.append({
                        'pid': process.info['pid'], 'name': process.info.get('name'),
                        'exe': process.info.get('exe'),
                    })
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                continue

        def safe_log_tail(path):
            try:
                with path.open('rb') as stream:
                    stream.seek(max(0, path.stat().st_size - 16384))
                    content = stream.read().decode('utf-8', errors='replace')
            except OSError:
                return None
            lines = [line for line in content.splitlines()
                     if re.search(r'presenter|panic|error|failed|slint', line, re.IGNORECASE)]
            scrubbed = []
            for line in lines[-20:]:
                line = re.sub(r'(?i)(password|token|secret|api[_-]?key|authorization)(\s*[:=]\s*)\S+',
                              r'\1\2<redacted>', line)
                line = re.sub(r'(?i)\bBearer\s+\S+', 'Bearer <redacted>', line)
                line = re.sub(r'https?://\S+', '<url>', line)
                scrubbed.append(line[:1000])
            return scrubbed

        crash_log = Path(tempfile.gettempdir()) / 'hls-downloader-presenter-crash.log'
        runtime_logs = []
        for folder in (runtime / 'data', runtime / 'data' / 'logs', runtime / 'logs'):
            if folder.is_dir():
                for path in sorted(folder.glob('*.log'), key=lambda item: item.stat().st_mtime, reverse=True)[:5]:
                    runtime_logs.append({'path': str(path), 'tail': safe_log_tail(path)})
        print(json.dumps({
            'diagnostic': 'presenter-window-not-found', 'expected_title': title,
            'startup_self_test': startup_diagnostic,
            'runtime': str(runtime),
            'runtime_presenter_processes': [
                {'pid': process['pid'], 'exe': process.get('exe')} for process in presenters
            ],
            'all_presenter_processes': all_presenter_processes,
            'presenter_windows': windows,
            'presenter_crash_log': {'path': str(crash_log), 'tail': safe_log_tail(crash_log)},
            'presenter_trace_log': safe_log_tail(Path(os.environ['HLS_V7_PRESENTER_TRACE_FILE'])),
            'runtime_logs': runtime_logs,
        }, ensure_ascii=False), flush=True)
        _last_presenter_diagnostic = now
    return None


def confirmation_rendered(handle):
    rect = wintypes.RECT()
    scale = user32.GetDpiForWindow(handle) / 96
    user32.GetWindowRect(handle, ctypes.byref(rect))
    # HWND 出现早于首帧绘制；等待实际蓝色确认按钮，避免点击尚未布局的窗口。
    # 读取此 HWND 的绘制表面；屏幕抓取会漏掉 layered window 或读到遮挡窗口。
    surface = print_window(handle, rect.right-rect.left, rect.bottom-rect.top)
    def blue_button(area):
        return sum(blue > 150 and blue > red*1.4 and blue > green*1.15
            for red,green,blue in area.convert('RGB').getdata()) > 80*scale*scale

    if surface is not None:
        area = surface.crop((surface.width-round(102*scale), surface.height-round(60*scale),
            surface.width-round(8*scale), surface.height-round(10*scale))).convert('RGB')
        if blue_button(area):
            return True
    # Slint 的 PrintWindow 可只返回局部重绘区；只有屏幕采样点确属该窗才采用实际屏幕像素。
    user32.WindowFromPoint.argtypes = [wintypes.POINT]
    user32.WindowFromPoint.restype = wintypes.HWND
    user32.GetAncestor.argtypes = [wintypes.HWND, wintypes.UINT]
    user32.GetAncestor.restype = wintypes.HWND
    point = wintypes.POINT(rect.right-round(54*scale), rect.bottom-round(35*scale))
    if user32.GetAncestor(user32.WindowFromPoint(point), 2) != handle:
        return False
    return blue_button(ImageGrab.grab(bbox=(rect.right-round(102*scale), rect.bottom-round(60*scale),
        rect.right-round(8*scale), rect.bottom-round(10*scale)),
        include_layered_windows=True, all_screens=True))


def click_download_confirmation(accept=True, input_mode='system', evidence_path=None, runtime=None):
    handle = wait(lambda: presenter_window(runtime=runtime), 'Desktop confirmation did not appear', 10)
    # 隔离浏览器的系统通知位于右下角；将测试窗口移到左上角，保留真实按钮输入。
    user32.SetWindowPos(handle, wintypes.HWND(-1), 24, 12, 0, 0, 0x0001 | 0x0040)
    user32.SetForegroundWindow(handle)
    # Slint 只重绘变化区域；先正常悬停确认按钮，才能从 PrintWindow 读取该按钮的最新帧。
    hover_rect = wintypes.RECT()
    user32.GetWindowRect(handle, ctypes.byref(hover_rect))
    hover_scale = user32.GetDpiForWindow(handle) / 96
    user32.SetCursorPos(hover_rect.right-round(54*hover_scale), hover_rect.bottom-round(35*hover_scale))
    try:
        wait(lambda:confirmation_rendered(handle), 'Desktop confirmation button has not rendered', 8)
    except TimeoutError:
        if evidence_path:
            rect = wintypes.RECT()
            user32.GetWindowRect(handle, ctypes.byref(rect))
            capture(handle, Path(evidence_path).with_suffix('.png'))
            ImageGrab.grab(all_screens=True).save(Path(evidence_path).with_suffix('.desktop.png'))
            Path(evidence_path).write_text(json.dumps({
                'handle': handle, 'rendered': False,
                'bounds': [rect.left, rect.top, rect.right, rect.bottom],
                'scale': user32.GetDpiForWindow(handle) / 96,
                'visible': bool(user32.IsWindowVisible(handle)),
            }, indent=2), encoding='utf8')
        raise
    rect = wintypes.RECT()
    user32.GetWindowRect(handle, ctypes.byref(rect))
    scale = user32.GetDpiForWindow(handle) / 96
    # 窗口底部固定的“确认下载”按钮；真实鼠标输入，保留生产回调。
    user32.SetCursorPos(rect.right - round((54 if accept else 134) * scale), rect.bottom - round(35 * scale))
    user32.WindowFromPoint.argtypes = [wintypes.POINT]
    user32.WindowFromPoint.restype = wintypes.HWND
    user32.GetAncestor.argtypes = [wintypes.HWND, wintypes.UINT]
    user32.GetAncestor.restype = wintypes.HWND
    if input_mode == 'system':
        def ready_for_click():
            user32.SetWindowPos(handle, wintypes.HWND(-1), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0040)
            user32.SetForegroundWindow(handle)
            user32.GetWindowRect(handle, ctypes.byref(rect))
            user32.SetCursorPos(rect.right-round((54 if accept else 134)*scale), rect.bottom-round(35*scale))
            time.sleep(.1)
            point = wintypes.POINT()
            user32.GetCursorPos(ctypes.byref(point))
            return user32.GetAncestor(user32.WindowFromPoint(point), 2) == handle
        # 浏览器打开扩展窗后仍可能抢回前台；鼠标真正命中确认窗后才发送输入。
        try:
            wait(ready_for_click, 'Confirmation button is covered by another window', 5)
        except TimeoutError:
            if evidence_path:
                point = wintypes.POINT()
                user32.GetCursorPos(ctypes.byref(point))
                capture(handle, Path(evidence_path).with_suffix('.png'))
                Path(evidence_path).write_text(json.dumps({'handle': handle,
                    'bounds': [rect.left, rect.top, rect.right, rect.bottom],
                    'cursor': [point.x, point.y],
                    'hit_root_window': user32.GetAncestor(user32.WindowFromPoint(point), 2),
                    'foreground': user32.GetForegroundWindow(), 'covered': True}, indent=2), encoding='utf8')
            raise
    evidence = None
    if evidence_path:
        point = wintypes.POINT()
        user32.GetCursorPos(ctypes.byref(point))
        evidence = {'handle': handle, 'accept': accept, 'input_mode': input_mode,
                    'bounds': [rect.left, rect.top, rect.right, rect.bottom], 'scale': scale,
                    'cursor': [point.x, point.y], 'hit_window': user32.WindowFromPoint(point),
                    'foreground': user32.GetForegroundWindow()}
        evidence['hit_root_window'] = user32.GetAncestor(evidence['hit_window'], 2)
        capture(handle, Path(evidence_path).with_suffix('.png'))
    if input_mode == 'window':
        point=wintypes.POINT(rect.right-round((54 if accept else 134)*scale),rect.bottom-round(35*scale))
        user32.ScreenToClient(handle,ctypes.byref(point))
        position=(point.y << 16) | (point.x & 0xffff)
        for message, buttons in [(0x0200,0),(0x0201,1),(0x0202,0)]:
            assert user32.PostMessageW(handle,message,buttons,position), 'Native window input rejected'
            time.sleep(.08)
        return
    class MouseInput(ctypes.Structure):
        _fields_ = [('dx', wintypes.LONG), ('dy', wintypes.LONG), ('data', wintypes.DWORD),
                   ('flags', wintypes.DWORD), ('time', wintypes.DWORD), ('extra', ctypes.c_size_t)]
    class InputUnion(ctypes.Union):
        _fields_ = [('mouse', MouseInput)]
    class Input(ctypes.Structure):
        _fields_ = [('type', wintypes.DWORD), ('value', InputUnion)]
    user32.SendInput.argtypes = [wintypes.UINT, ctypes.POINTER(Input), ctypes.c_int]
    user32.SendInput.restype = wintypes.UINT
    for flags in (2, 4):
        event = Input(0, InputUnion(mouse=MouseInput(flags=flags)))
        assert user32.SendInput(1, ctypes.byref(event), ctypes.sizeof(Input)) == 1, 'System mouse input rejected'
        time.sleep(.08)
    user32.SetWindowPos(handle, wintypes.HWND(-2), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010)
    if evidence is not None:
        user32.GetWindowRect(handle, ctypes.byref(rect))
        evidence['after_bounds'] = [rect.left, rect.top, rect.right, rect.bottom]
        evidence['after_visible'] = bool(user32.IsWindowVisible(handle))
        Path(evidence_path).write_text(json.dumps(evidence, indent=2), encoding='utf8')


def run(args):
    args.report.parent.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'browser': args.browser, 'page': PAGE, 'checks': []}
    report['installed_sha256'] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
        for p in [args.runtime/'app/HLSDownloaderDesktop-7.0.3.jar',
                  args.runtime/'app/resources/HLSDownloaderEngine.exe',
                  args.runtime/'app/resources/HLSDownloaderPresenter.exe']}
    driver = None
    stress_host = None
    created = set()
    initial = set()
    unique_url = MEDIA + '?hls-confirmation=' + uuid.uuid4().hex

    def core(request):
        pipe = Pipe()
        try:
            return pipe.request(request)
        finally:
            pipe.close()

    def command(value):
        return core({'type': 'command', 'request_id': 1, 'command': value})

    def tasks():
        current = core({'type': 'snapshot', 'request_id': 1})['tasks']
        for task in current:
            if task['task_id'] not in initial and task['url'] == unique_url:
                created.add(task['task_id'])
        return current

    def record(name, actual=True):
        report['checks'].append({'name': name, 'actual': actual})
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
        print(json.dumps({'check': name}, ensure_ascii=True), flush=True)

    def popup_window(title='确认下载'):
        return presenter_window(title, args.runtime)

    def reject_confirmation():
        click_download_confirmation(accept=False, runtime=args.runtime,
            evidence_path=args.report.with_name(f'cancel-{len(report["checks"])}.json'))

    def media_button():
        return driver.execute_script("return document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector('.video-download:not(.identifying)') || null")

    def click_media():
        button = wait(lambda: (b if (b := media_button()) and b.is_enabled() else None), 'Media action unavailable', 15)
        started = time.perf_counter()
        ActionChains(driver).move_to_element(button).click().perform()
        handle = wait(popup_window, 'Web click did not display desktop confirmation', 10)
        latency = (time.perf_counter() - started) * 1000
        capture(handle, args.report.with_name(f'confirmation-{len(report["checks"])}.png'))
        assert not any(t['task_id'] not in initial and t['url'] == unique_url for t in tasks()), 'Task started before confirmation'
        record('public-media-click-shows-confirmation-before-task', {'latency_ms': round(latency, 2)})

    with tempfile.TemporaryDirectory(prefix='browser-confirmation-', dir=args.runtime.resolve().parents[1] / 'test-tmp') as temporary:
        stage = Path(temporary)
        try:
            initial = {t['task_id'] for t in tasks()}
            if args.cold_ui:
                stopped = []
                for process in runtime_processes(args.runtime.resolve()):
                    if process['name'] in ('HLSDownloader.exe', 'HLSDownloaderPresenter.exe'):
                        try:
                            p = psutil.Process(process['pid'])
                            p.terminate()
                            stopped.append(p)
                        except psutil.NoSuchProcess:
                            pass
                _, alive = psutil.wait_procs(stopped, timeout=10)
                assert not alive, 'Workbench or Presenter did not stop'
                record('workbench-and-presenter-closed-before-browser-click')
            addon = stage / 'extension.xpi'
            with zipfile.ZipFile(addon, 'w', zipfile.ZIP_DEFLATED) as archive:
                for source in args.extension.resolve().rglob('*'):
                    if source.is_file():
                        archive.write(source, source.relative_to(args.extension.resolve()).as_posix())
            if args.browser == 'firefox':
                options = Options()
                options.binary_location = str(_find_firefox())
                options.page_load_strategy = 'eager'
                if not args.headed:
                    options.add_argument('-headless')
                options.set_preference('media.autoplay.default', 0)
                options.set_preference('media.autoplay.blocking_policy', 0)
                options.set_preference('browser.shell.checkDefaultBrowser', False)
                service = Service(str(args.driver), service_args=['--profile-root', str(stage), '--allow-system-access'], log_output=str(args.report.with_suffix('.gecko.log')))
                print('Starting test Firefox', flush=True)
                driver = webdriver.Firefox(options=options, service=service)
                print('Installing candidate extension in test Firefox', flush=True)
                driver.install_addon(str(addon), temporary=True)
            else:
                options = EdgeOptions()
                options.binary_location = str(_find_edge())
                options.page_load_strategy = 'eager'
                for argument in [f'--user-data-dir={stage / "profile"}',
                        '--disable-features=DisableLoadExtensionCommandLineSwitch',
                        f'--load-extension={args.extension.resolve()}', '--no-first-run',
                        '--autoplay-policy=no-user-gesture-required']:
                    options.add_argument(argument)
                if not args.headed:
                    options.add_argument('--headless=new')
                driver = webdriver.Edge(options=options, service=EdgeService(str(args.driver)))
            _fit_browser_window(driver)
            driver.set_page_load_timeout(40)
            print('Opening public video page', flush=True)
            driver.get(PAGE)
            def find_video():
                driver.switch_to.default_content()
                if driver.find_elements(By.TAG_NAME, 'video'):
                    return True
                for frame in driver.find_elements(By.TAG_NAME, 'iframe'):
                    driver.switch_to.frame(frame)
                    if driver.find_elements(By.TAG_NAME, 'video'):
                        return True
                    driver.switch_to.default_content()
                return False
            wait(find_video, 'Public page has no video', 15)
            driver.execute_script('const v=document.querySelector("video");v.src=arguments[0];v.muted=true;v.loop=true;v.play();', unique_url)
            wait(lambda: driver.execute_script('return document.querySelector("video").currentTime > 0'), 'Public video did not play', 20)
            click_media()
            if args.cold_ui:
                assert not any(p['name'] == 'HLSDownloader.exe' for p in runtime_processes(args.runtime.resolve()))
                record('browser-wakes-confirmation-without-starting-workbench')
            reject_confirmation()
            wait(lambda: not popup_window(), 'Cancel did not close confirmation', 5)
            wait(lambda: driver.execute_script("const b=document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector('.video-download');return b && !b.disabled"), 'Cancel left webpage download disabled', 8)
            assert not created
            record('desktop-cancel-leaves-no-task-and-allows-retry')
            click_media()
            click_download_confirmation(runtime=args.runtime,
                evidence_path=args.report.with_name(f'accept-{len(report["checks"])}.json'))

            def completed():
                task = next((t for t in tasks() if t['url'] == unique_url and t['task_id'] not in initial), None)
                if task and task['status'] == 'failed':
                    raise AssertionError(task.get('error_message'))
                return task if task and task['status'] == 'completed' else None

            task = wait(completed, 'Confirmed public media did not finish downloading', 45)
            assert hashlib.sha256(Path(task['output_path']).read_bytes()).hexdigest() == SHA256
            record('actual-confirm-button-downloads-public-file-with-correct-sha256', {'bytes': task['downloaded_bytes'], 'sha256': SHA256})
            record('completed-notification-visible', bool(wait(lambda: popup_window('下载完成'), 'Completion popup missing', 10)))
            wait(lambda: driver.execute_script("const b=document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector('.video-download');return b && !b.disabled"), 'Completed download left webpage action permanently disabled', 8)
            record('completed-webpage-action-becomes-available-again')
            # 工具栏弹窗与网页按钮共用同一桌面确认链路，重复的已完成任务仍应可见。
            popup_url = 'chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/popup.html'
            if args.browser == 'firefox':
                driver.set_context('chrome')
                addon_id = json.loads((args.extension / 'manifest.json').read_text(encoding='utf8'))['browser_specific_settings']['gecko']['id']
                extension_uuid = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))[addon_id]
                driver.set_context('content')
                popup_url = f'moz-extension://{extension_uuid}/popup.html'
            driver.switch_to.new_window('tab')
            driver.get(popup_url)
            button = wait(lambda: next((b for b in driver.find_elements(By.TAG_NAME, 'button') if b.is_displayed() and b.text.strip() == '下载'), None), 'Popup media action missing', 15)
            button.click()
            handle = wait(popup_window, 'Popup download did not display confirmation', 10)
            capture(handle, args.report.with_name('popup-confirmation.png'))
            reject_confirmation()
            wait(lambda: not popup_window(), 'Popup cancel did not close confirmation', 5)
            record('toolbar-popup-also-opens-desktop-confirmation')
            if args.confirmation_stress:
                from smoke_v7_presenter import native_message
                stress_host = subprocess.Popen([str(args.runtime/'app/resources/HLSDownloaderNativeHost.exe')],
                    cwd=args.runtime, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                    creationflags=subprocess.CREATE_NO_WINDOW)
                for index in range(args.confirmation_stress):
                    rect=wintypes.RECT()
                    user32.GetWindowRect(handle,ctypes.byref(rect))
                    scale=user32.GetDpiForWindow(handle)/96
                    x,y=rect.right-round(54*scale),rect.bottom-round(35*scale)
                    hud=popup_window('下载完成') or popup_window('下载进度')
                    if hud:
                        # 将旧 HUD 标题栏放到下一确认按钮的物理位置；鼠标位置保持不变。
                        user32.SetWindowPos(hud,None,x-round(60*scale),y-round(15*scale),0,0,0x0001|0x0010)
                        user32.SetCursorPos(x,y)
                        time.sleep(.15)
                        user32.PostMessageW(hud,0x0010,0,0)
                        wait(lambda:not user32.IsWindowVisible(hud),'Previous HUD did not close',5)
                    unique_url=MEDIA+'?hls-confirmation='+uuid.uuid4().hex
                    response=native_message(stress_host,{'op':'offer','resource':{
                        'url':unique_url,'filename':f'confirm-stress-{index}.webm','title':'Native pointer lifecycle',
                        'resource_kind':'file','mime_type':'video/webm','source_page_url':PAGE,
                        'client_request_id':uuid.uuid4().hex}})
                    assert response.get('ok'),response
                    handle=wait(popup_window,'Lifecycle confirmation missing',10)
                    before=wintypes.RECT()
                    user32.GetWindowRect(handle,ctypes.byref(before))
                    capture(handle,args.report.with_name(f'stress-{index}-before.png'))
                    click_download_confirmation(runtime=args.runtime)
                    task=wait(completed,'Unmoved pointer failed to confirm download',30)
                    assert hashlib.sha256(Path(task['output_path']).read_bytes()).hexdigest()==SHA256
                    wait(lambda:popup_window('下载完成'),'Lifecycle completion HUD missing',10)
                    record('hud-to-confirmation-without-cursor-movement-downloads-exact-bytes',
                           {'cycle':index,'before_bounds':[before.left,before.top,before.right,before.bottom],
                            'task_id':task['task_id'],'sha256':SHA256,'previous_hud_repositioned':bool(hud)})
            report['passed'] = True
        except Exception as error:
            report['error'] = str(error)
            report['runtime_processes_at_failure'] = runtime_processes(args.runtime.resolve())
            report['handoffs_at_failure'] = [json.loads(item) for item in core({'type': 'load_handoffs', 'request_id': 1})['items'] if json.loads(item).get('offer', {}).get('url') == unique_url]
            if driver:
                driver.save_screenshot(str(args.report.with_name('failure.png')))
            raise
        finally:
            if stress_host:
                stress_host.terminate()
                stress_host.wait(timeout=10)
            if driver:
                driver.quit()
            for encoded in core({'type': 'load_handoffs', 'request_id': 1})['items']:
                handoff = json.loads(encoded)
                if handoff.get('status') == 'pending' and handoff.get('offer', {}).get('url') == unique_url:
                    command({'kind': 'reject_handoff', 'handoff_id': handoff['id'], 'suppress_site_kind': False})
            # 只删除本次唯一地址创建的任务，绝不按编号范围删除用户任务。
            for task in tasks():
                if task['task_id'] in created:
                    command({'kind': 'task_action', 'task_id': task['task_id'], 'action': 'delete_files'})
            report['cleanup_preserved_original_task_ids'] = initial <= {t['task_id'] for t in tasks()}
            if args.cold_ui:
                subprocess.Popen([str(args.runtime.resolve() / 'HLSDownloader.exe')], cwd=args.runtime.resolve(), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                report['workbench_restart_requested'] = True
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--browser', choices=['firefox', 'edge'], default='firefox')
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--extension', type=Path, required=True)
    parser.add_argument('--driver', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--cold-ui', action='store_true')
    parser.add_argument('--headed', action='store_true')
    parser.add_argument('--confirmation-stress', type=int, default=0)
    print(json.dumps(run(parser.parse_args()), ensure_ascii=False, indent=2))
