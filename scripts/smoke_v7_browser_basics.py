"""Real browser -> installed Native Host -> desktop confirmation -> media file."""
from __future__ import annotations

import argparse
import base64
import contextlib
import ctypes
import hashlib
from http.server import BaseHTTPRequestHandler, SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import re
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import uuid
import zipfile
from urllib.parse import quote
from urllib.request import urlopen

import websocket
import psutil

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys
from selenium.webdriver.common.action_chains import ActionChains
from selenium.webdriver.support.ui import Select
from selenium.common.exceptions import StaleElementReferenceException
from selenium.webdriver.firefox.options import Options as FirefoxOptions
from selenium.webdriver.firefox.service import Service as FirefoxService
from selenium.webdriver.edge.options import Options as EdgeOptions
from selenium.webdriver.edge.service import Service as EdgeService
from ctypes import wintypes

from smoke_extension_media import _find_firefox, _find_edge, _fit_browser_window, _open_first_media_actions
from smoke_v7_browser_download_confirmation import click_download_confirmation, presenter_window
from smoke_v7_presenter_visual import user32 as native_user32
from smoke_v7_portable_app import Pipe, wait

PUBLIC = [
    ('hls', 'https://hlsjs.video-dev.org/demo/?src=https%3A%2F%2Ftest-streams.mux.dev%2Fx36xhzz%2Fx36xhzz.m3u8'),
    ('dash', 'https://reference.dashif.org/dash.js/latest/samples/getting-started/manual-load-single-video.html'),
]


def choose_browser_window(user32, windows, expected_title):
    user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
    user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    candidates = []
    for hwnd in windows:
        length = user32.GetWindowTextLengthW(hwnd)
        title = ctypes.create_unicode_buffer(length + 1)
        user32.GetWindowTextW(hwnd, title, len(title))
        rect = wintypes.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
        title_match = bool(expected_title and expected_title in title.value)
        area = max(0, rect.right - rect.left) * max(0, rect.bottom - rect.top)
        candidates.append((title_match, area, hwnd))
    assert candidates, 'Native browser window missing'
    return max(candidates, key=lambda item: (item[0], item[1]))[2]


def focus_browser_titlebar(user32, browser_hwnd, browser_pid, description, failure_screenshot=None):
    user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    user32.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int,
                                    ctypes.c_int, ctypes.c_int, wintypes.UINT]
    user32.SetWindowPos.restype = wintypes.BOOL
    user32.WindowFromPoint.argtypes = [wintypes.POINT]
    user32.WindowFromPoint.restype = wintypes.HWND
    user32.GetAncestor.argtypes = [wintypes.HWND, wintypes.UINT]
    user32.GetAncestor.restype = wintypes.HWND
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.GetWindowThreadProcessId.restype = wintypes.DWORD
    user32.GetForegroundWindow.restype = wintypes.HWND
    user32.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
    user32.SetCursorPos.restype = wintypes.BOOL
    user32.mouse_event.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.DWORD,
                                   wintypes.DWORD, ctypes.c_size_t]

    user32.ShowWindow(browser_hwnd, 9)
    assert user32.SetWindowPos(browser_hwnd, wintypes.HWND(0), 0, 0, 0, 0,
                               0x0001 | 0x0002 | 0x0040), (description, 'SetWindowPos(HWND_TOP) failed')
    rect = wintypes.RECT()
    assert user32.GetWindowRect(browser_hwnd, ctypes.byref(rect)), (description, 'GetWindowRect failed')
    point = wintypes.POINT((rect.left + rect.right) // 2, rect.top + 8)
    assert user32.SetCursorPos(point.x, point.y), (description, 'SetCursorPos failed', point.x, point.y)
    hit = user32.WindowFromPoint(point)
    root = user32.GetAncestor(hit, 2) if hit else None
    owner = wintypes.DWORD()
    if root:
        user32.GetWindowThreadProcessId(root, ctypes.byref(owner))
    if root != browser_hwnd or owner.value != browser_pid:
        screenshot = None
        if failure_screenshot is not None:
            from PIL import ImageGrab
            ImageGrab.grab().save(failure_screenshot)
            screenshot = str(failure_screenshot)
        raise AssertionError((
            description, 'titlebar point is outside the selected browser window',
            {'browser_hwnd': int(browser_hwnd), 'hit_hwnd': int(hit) if hit else None,
             'root_hwnd': int(root) if root else None, 'root_pid': owner.value,
             'point': [point.x, point.y], 'failure_screenshot': screenshot},
        ))
    user32.mouse_event(0x0002, 0, 0, 0, 0)
    user32.mouse_event(0x0004, 0, 0, 0, 0)
    wait(lambda: user32.GetForegroundWindow() == browser_hwnd,
         description + ' browser did not become foreground after titlebar click', 5)


def run(args):
    runtime = args.runtime.resolve()
    extension = args.extension.resolve()
    report = {'passed': False, 'browser': args.browser, 'checks': [], 'components': {},
              'test_script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    for binary in ['HLSDownloaderEngine.exe', 'HLSDownloaderNativeHost.exe', 'HLSDownloaderPresenter.exe']:
        with (runtime / 'app/resources' / binary).open('rb') as stream:
            report['components'][binary] = hashlib.file_digest(stream, 'sha256').hexdigest()
    report['extension_sha256'] = {str(p.relative_to(extension)): hashlib.sha256(p.read_bytes()).hexdigest()
                                  for p in extension.rglob('*') if p.is_file()}
    created = set()
    driver = None
    selection_server = None
    video_server = None

    def core(message):
        p = Pipe()
        try:
            return p.request(message)
        finally:
            p.close()

    def tasks():
        return core({'type': 'snapshot', 'request_id': 1})['tasks']

    # 并行识别测试不拥有下载任务，只以保存的用户基线核验数据保留。
    baseline = json.loads((args.report.parent / 'user-tasks-before.json').read_text(encoding='utf8'))
    initial = {t['task_id'] for t in baseline['tasks']}

    def record(name, actual=True):
        report['checks'].append({'name': name, 'actual': actual})
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
        print(json.dumps({'check': name, 'actual': actual}, ensure_ascii=True), flush=True)

    def shadow(selector):
        return driver.execute_script('return document.querySelector("hls-downloader-media-panel")?.shadowRoot?.querySelector(arguments[0]) || null', selector)

    def extension_message(value):
        return driver.execute_async_script('const [m,done]=arguments;(globalThis.browser||chrome).runtime.sendMessage(m).then(done,e=>done({ok:false,error:String(e)}))', value)

    def edge_context_menu_command(label, browser_pid, retry_right_click=None, browser_hwnd=None):
        from PIL import ImageGrab
        def menu_item_controls(handle):
            menu_script = f"""
[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$handle=[IntPtr]([long]{handle})
$browserPid=[int]{browser_pid}
$condition=New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty,[System.Windows.Automation.ControlType]::MenuItem)
try {{$root=[System.Windows.Automation.AutomationElement]::FromHandle($handle);$items=$root.FindAll([System.Windows.Automation.TreeScope]::Descendants,$condition)}} catch {{$items=@()}}
$matches=@(
    foreach($item in $items){{$c=$item.Current;if($c.ProcessId -eq $browserPid){{[pscustomobject]@{{name=$c.Name;pid=$c.ProcessId;x=$c.BoundingRectangle.X;y=$c.BoundingRectangle.Y;width=$c.BoundingRectangle.Width;height=$c.BoundingRectangle.Height;enabled=$c.IsEnabled}}}}}}
)
ConvertTo-Json -InputObject @($matches) -Compress
"""
            output = subprocess.check_output(
                ['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', menu_script],
                timeout=45, creationflags=subprocess.CREATE_NO_WINDOW)
            items = json.loads(output.decode('utf8', errors='replace') or '[]') or []
            return items if isinstance(items, list) else [items]

        def context_menu_windows():
            native_user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
            native_user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
            native_user32.GetWindowLongPtrW.argtypes = [wintypes.HWND, ctypes.c_int]
            native_user32.GetWindowLongPtrW.restype = ctypes.c_ssize_t
            native_user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
            native_user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
            native_user32.GetWindow.argtypes = [wintypes.HWND, wintypes.UINT]
            native_user32.GetWindow.restype = wintypes.HWND
            native_user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]

            candidates = []
            @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
            def find_menu(candidate, _):
                owner = wintypes.DWORD()
                native_user32.GetWindowThreadProcessId(candidate, ctypes.byref(owner))
                class_name = ctypes.create_unicode_buffer(256)
                native_user32.GetClassNameW(candidate, class_name, len(class_name))
                style = native_user32.GetWindowLongPtrW(candidate, -16)
                if (owner.value != browser_pid or not native_user32.IsWindowVisible(candidate)
                        or (browser_hwnd is not None and int(candidate) == int(browser_hwnd))):
                    return True
                if class_name.value != '#32768' and not (
                        class_name.value == 'Chrome_WidgetWin_1' and style & 0x80000000):
                    return True
                title = ctypes.create_unicode_buffer(native_user32.GetWindowTextLengthW(candidate) + 1)
                native_user32.GetWindowTextW(candidate, title, len(title))
                bounds = wintypes.RECT()
                native_user32.GetWindowRect(candidate, ctypes.byref(bounds))
                candidates.append({
                    'hwnd': int(candidate), 'pid': owner.value, 'class': class_name.value,
                    'title': title.value, 'style': int(style),
                    'owner_hwnd': int(native_user32.GetWindow(candidate, 4) or 0),
                    'bounds': [bounds.left, bounds.top, bounds.right, bounds.bottom],
                })
                return True
            native_user32.EnumWindows(find_menu, 0)
            for candidate in candidates:
                candidate['uia_menu_items'] = menu_item_controls(candidate['hwnd'])
                candidate['is_menu'] = candidate['class'] == '#32768' or bool(candidate['uia_menu_items'])
            return candidates

        def context_menu_handles():
            return [window['hwnd'] for window in context_menu_windows() if window['is_menu']]

        def menu_items():
            for window in context_menu_windows():
                if not window['is_menu']:
                    continue
                items = window['uia_menu_items']
                if any(entry['pid'] == browser_pid
                       and (entry['name'] == label or 'HLS Downloader' in entry['name'])
                       for entry in items):
                    return items
            return []

        try:
            items = wait(lambda: (found if (found := menu_items()) else None),
                         'Native Edge context menu did not open', 5)
        except TimeoutError as error:
            if str(error) != 'Native Edge context menu did not open':
                raise
            if retry_right_click is None:
                failure_image = args.report.with_name('native-context-menu-failure.png')
                ImageGrab.grab().save(failure_image)
                report['context_menu_failure_screenshot'] = str(failure_image)
                args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
                raise
            stale_menus = context_menu_handles()
            report.setdefault('edge_context_menu_candidates', []).extend(context_menu_windows())
            if stale_menus:
                native_user32.keybd_event(0x1B, 0, 0, 0)
                native_user32.keybd_event(0x1B, 0, 2, 0)
                wait(lambda: not context_menu_handles(),
                     'Native Edge context menu remained open after Escape', 3)
                report.setdefault('edge_context_menu_dismissals', []).append(
                    {'handles': stale_menus, 'closed_before_retry': True})
            retry_right_click()
            try:
                items = wait(lambda: (found if (found := menu_items()) else None),
                             'Native Edge context menu did not open after one real right-click retry', 5)
            except TimeoutError:
                from PIL import ImageGrab
                failure_image = args.report.with_name('native-context-menu-failure.png')
                ImageGrab.grab().save(failure_image)
                report['context_menu_failure_screenshot'] = str(failure_image)
                args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
                raise
        report.setdefault('context_menu_items', []).append(items)
        item = next((entry for entry in items if entry['pid'] == browser_pid and entry['name'] == label), None)
        if item is None:
            parent = next((entry for entry in items if entry['pid'] == browser_pid and 'HLS Downloader' in entry['name'] and entry['width'] > 0), None)
            assert parent, ('Native Edge extension menu missing', browser_pid, items)
            parent_x = round(parent['x'] + parent['width'] / 2)
            parent_y = round(parent['y'] + parent['height'] / 2)
            assert native_user32.SetCursorPos(parent_x, parent_y), ('Edge extension menu cursor move failed', parent_x, parent_y)

            def submenu_item():
                return next((entry for entry in menu_items()
                             if entry['pid'] == browser_pid and entry['name'] == label and entry['width'] > 0), None)

            try:
                item = wait(submenu_item, 'Native Edge submenu did not open on hover: ' + label, 2)
            except TimeoutError:
                # Edge's generated extension group can require a click to expand.
                native_user32.mouse_event(2, 0, 0, 0, 0)
                native_user32.mouse_event(4, 0, 0, 0, 0)
                try:
                    item = wait(submenu_item, 'Native Edge submenu missing: ' + label, 15)
                except TimeoutError:
                    report.setdefault('context_menu_items', []).append(menu_items())
                    failure_image = args.report.with_name('native-context-submenu-failure.png')
                    ImageGrab.grab().save(failure_image)
                    report['context_menu_failure_screenshot'] = str(failure_image)
                    report['context_menu_parent_expansion'] = {
                        'parent': parent, 'cursor': [parent_x, parent_y],
                    }
                    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
                    raise
            ImageGrab.grab().save(args.report.with_name('native-context-menu.png'))
            report.setdefault('context_menu_items', []).append(menu_items())
        assert item['enabled'] and item['width'] > 0 and item['height'] > 0, item
        report.setdefault('context_controls', []).append(item)
        native_user32.SetCursorPos(round(item['x'] + item['width'] / 2), round(item['y'] + item['height'] / 2))
        native_user32.mouse_event(2, 0, 0, 0, 0)
        native_user32.mouse_event(4, 0, 0, 0, 0)

    def toolbar_state(scroll_target=None):
        # 原生弹窗在独立进程中，Marionette 不能把 XUL browser 当 HTML iframe 切换。
        source = """const target=SCROLL_TARGET;
        if(target){
            const button=[...content.document.querySelectorAll('button')].find(b=>
                (target.text===null || b.textContent.trim()===target.text) &&
                (target.title===null || b.title===target.title));
            if(button)button.scrollIntoView({block:'start',inline:'nearest'});
        }
        sendAsyncMessage('hls-qa-popup', {
            ready:content.document.documentElement.dataset.popupReady,
            text:content.document.body.textContent,
            scale:content.devicePixelRatio,
            viewport:{width:content.innerWidth,height:content.innerHeight},
            buttons:[...content.document.querySelectorAll('button')].map(b=>{const r=b.getBoundingClientRect();return {text:b.textContent.trim(),title:b.title,pressed:b.getAttribute('aria-pressed'),disabled:b.disabled,visible:r.left>=0&&r.top>=0&&r.right<=content.innerWidth&&r.bottom<=content.innerHeight,rect:{left:r.left,top:r.top,right:r.right,bottom:r.bottom},x:content.mozInnerScreenX+r.x+r.width/2,y:content.mozInnerScreenY+r.y+r.height/2}})
        });"""
        source = source.replace('SCROLL_TARGET', json.dumps(scroll_target))
        return driver.execute_async_script("""const [url,done]=arguments;
            const b=[...document.querySelectorAll('panel browser')].find(b=>b.getAttribute('webextension-view-type')==='popup' && b.closest('panel')?.state==='open');
            if(!b){done({ready:'closed'});return}
            const mm=b.messageManager;
            const listener=m=>{clearTimeout(timer);mm.removeMessageListener('hls-qa-popup',listener);done(m.data)};
            const timer=setTimeout(()=>{mm.removeMessageListener('hls-qa-popup',listener);done({ready:'unavailable'})},1500);
            mm.addMessageListener('hls-qa-popup',listener);mm.loadFrameScript(url,false);
        """, 'data:application/javascript,' + quote(source))

    with tempfile.TemporaryDirectory(prefix='browser-basics-', dir=runtime.parents[1] / 'test-tmp') as temporary:
        stage = Path(temporary)
        try:
            if args.stage in {'toolbar', 'entries', 'errors'}:
                fixture_root = stage / 'video-fixture'
                fixture_root.mkdir()
                video_path = fixture_root / 'stream.mp4'
                subprocess.run([
                    str(runtime / 'app/resources/ffmpeg.exe'), '-hide_banner', '-loglevel', 'error', '-y',
                    '-f', 'lavfi', '-i', 'testsrc=size=640x360:rate=30',
                    '-f', 'lavfi', '-i', 'sine=frequency=880:sample_rate=48000', '-t', '4',
                    '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-g', '30', '-c:a', 'aac',
                    '-movflags', '+faststart', str(video_path),
                ], check=True, timeout=60, capture_output=True)
                fixture_sha256 = hashlib.sha256(video_path.read_bytes()).hexdigest()
                for name in ('flower.mp4', 'flower-copy.mp4'):
                    (fixture_root / name).write_bytes(video_path.read_bytes())
                (fixture_root / 'index.html').write_text(
                    '<!doctype html><meta charset="utf-8"><title>Local media fixture</title>'
                    '<video controls autoplay muted loop width="640" height="360" src="/stream.mp4"></video>',
                    encoding='utf-8',
                )

                class FixtureHandler(SimpleHTTPRequestHandler):
                    def log_message(self, _format, *_args):
                        pass

                video_server = ThreadingHTTPServer(
                    ('127.0.0.1', 0),
                    lambda *handler_args, **handler_kwargs: FixtureHandler(
                        *handler_args, directory=str(fixture_root), **handler_kwargs
                    ),
                )
                threading.Thread(target=video_server.serve_forever, daemon=True).start()
                fixture_url = f'http://127.0.0.1:{video_server.server_port}'
            if args.browser == 'firefox':
                addon = stage / 'extension.xpi'
                with zipfile.ZipFile(addon, 'w', zipfile.ZIP_DEFLATED) as archive:
                    for p in extension.rglob('*'):
                        if not p.is_file(): continue
                        if args.stage == 'errors' and p.name == 'manifest.json':
                            manifest = json.loads(p.read_text(encoding='utf8'))
                            manifest['browser_specific_settings']['gecko']['id'] = 'hls-qa-unregistered@example.test'
                            archive.writestr('manifest.json', json.dumps(manifest))
                        else: archive.write(p, p.relative_to(extension).as_posix())
                options = FirefoxOptions()
                options.binary_location = str(_find_firefox())
                options.set_preference('browser.shell.checkDefaultBrowser', False)
                options.set_preference('browser.aboutwelcome.enabled', False)
                options.set_preference('browser.newtabpage.activity-stream.asrouter.providers.onboarding', '{"id":"onboarding","type":"local","localProvider":"OnboardingMessageProvider","enabled":false}')
                options.set_preference('browser.newtabpage.activity-stream.asrouter.providers.messaging-experiments', '{"id":"messaging-experiments","type":"remote-experiments","enabled":false}')
                options.set_preference('media.autoplay.default', 0)
                options.page_load_strategy = 'eager'
                service = FirefoxService(str(args.driver), service_args=['--profile-root', str(stage), '--allow-system-access'], log_output=str(args.report.with_suffix('.gecko.log')))
                driver = webdriver.Firefox(options=options, service=service)
                addon_id = driver.install_addon(str(addon), temporary=True)
                driver.set_context('chrome')
                extension_uuid = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))[addon_id]
                driver.set_context('content')
                popup_url = f'moz-extension://{extension_uuid}/popup.html'
            else:
                edge_extension = extension
                edge_extension_id = 'bbdfldcjnikaemnimalegbopgaknjhla'
                if args.stage == 'errors':
                    from cryptography.hazmat.primitives import serialization
                    from cryptography.hazmat.primitives.asymmetric import rsa

                    edge_extension = stage / 'unregistered-extension'
                    shutil.copytree(extension, edge_extension)
                    public_key = rsa.generate_private_key(public_exponent=65537, key_size=2048).public_key().public_bytes(
                        encoding=serialization.Encoding.DER,
                        format=serialization.PublicFormat.SubjectPublicKeyInfo,
                    )
                    manifest_path = edge_extension / 'manifest.json'
                    manifest = json.loads(manifest_path.read_text(encoding='utf8'))
                    manifest['key'] = base64.b64encode(public_key).decode('ascii')
                    manifest_path.write_text(json.dumps(manifest), encoding='utf8')
                    key_hash = hashlib.sha256(public_key).digest()[:16]
                    edge_extension_id = ''.join(chr(ord('a') + (value >> 4)) + chr(ord('a') + (value & 0x0f))
                                                for value in key_hash)
                    report['test_extension_id'] = edge_extension_id
                    report['test_manifest_sha256'] = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
                options = EdgeOptions()
                options.binary_location = str(_find_edge())
                options.add_argument(f'--user-data-dir={stage / "profile"}')
                options.add_argument('--disable-features=DisableLoadExtensionCommandLineSwitch')
                options.add_argument(f'--load-extension={edge_extension}')
                options.add_argument('--autoplay-policy=no-user-gesture-required')
                options.add_argument('--no-first-run')
                options.add_argument('--enable-unsafe-extension-debugging')
                options.add_argument('--force-renderer-accessibility')
                options.page_load_strategy = 'eager'
                driver = webdriver.Edge(options=options, service=EdgeService(str(args.driver)))
                popup_url = f'chrome-extension://{edge_extension_id}/popup.html'
            _fit_browser_window(driver)
            driver.set_page_load_timeout(45)
            driver.set_script_timeout(20)
            if args.stage == 'toolbar' and args.browser == 'edge':
                from smoke_extension_subtitles import trigger_action
                page = fixture_url + '/index.html'
                driver.get(page)
                def frame_video():
                    driver.switch_to.default_content()
                    found = driver.find_elements(By.TAG_NAME, 'video')
                    if found:
                        return found[0]
                    for frame in driver.find_elements(By.TAG_NAME, 'iframe'):
                        driver.switch_to.default_content()
                        driver.switch_to.frame(frame)
                        found = driver.find_elements(By.TAG_NAME, 'video')
                        if found:
                            return found[0]
                    return None
                video = wait(frame_video, 'Public video missing', 20)
                driver.execute_script('arguments[0].muted=true;arguments[0].play().catch(()=>{})', video)
                wait(lambda: shadow('.video-download:not(.identifying)'), 'Public video not recognized', 30)
                driver.switch_to.default_content()
                address = driver.capabilities['ms:edgeOptions']['debuggerAddress']
                popup_connection = None
                cdp_id = 0

                def popup_call(method, params):
                    nonlocal cdp_id
                    cdp_id += 1
                    popup_connection.send(json.dumps({'id': cdp_id, 'method': method, 'params': params}))
                    while True:
                        response = json.loads(popup_connection.recv())
                        if response.get('id') == cdp_id:
                            assert 'error' not in response, response
                            return response['result']

                def popup_value(expression):
                    result = popup_call('Runtime.evaluate', {'expression': expression, 'returnByValue': True})
                    assert 'exceptionDetails' not in result, result
                    return result['result'].get('value')

                def edge_popup_targets():
                    with urlopen('http://' + address + '/json/list', timeout=3) as response:
                        targets = json.load(response)
                        report['last_browser_targets'] = [{k: t.get(k) for k in ('type', 'url', 'title')} for t in targets]
                        return [t for t in targets if t.get('url') == popup_url]

                def open_edge_toolbar():
                    nonlocal popup_connection
                    if popup_connection:
                        popup_connection.close()
                    trigger_action(int(address.rsplit(':', 1)[1]), 'bbdfldcjnikaemnimalegbopgaknjhla', page)
                    target = wait(lambda: next(iter(edge_popup_targets()), None), 'Native Edge popup missing', 10)
                    report.setdefault('native_popup_targets', []).append({k: target.get(k) for k in ('type', 'url', 'title')})
                    popup_connection = websocket.create_connection(target['webSocketDebuggerUrl'], timeout=5, suppress_origin=True)
                    wait(lambda: popup_value("document.documentElement.dataset.popupReady==='ready'"), 'Edge popup not ready', 15)

                def click_edge_popup(text=None, title=None):
                    selector = 'b.textContent.trim()===' + json.dumps(text) if text else 'b.title===' + json.dumps(title)
                    popup_value("(()=>{const b=[...document.querySelectorAll('button')].find(b=>" + selector + ");if(b)b.scrollIntoView({block:'nearest',inline:'nearest'})})()")
                    point = wait(lambda: popup_value("(()=>{const b=[...document.querySelectorAll('button')].find(b=>" + selector + ");if(!b||b.disabled)return null;const r=b.getBoundingClientRect();if(r.left<0||r.top<0||r.right>innerWidth||r.bottom>innerHeight)return null;return {x:r.x+r.width/2,y:r.y+r.height/2}})()"), 'Edge popup target remains outside viewport', 5)
                    assert point, (text, title)
                    popup_call('Input.dispatchMouseEvent', {'type': 'mousePressed', 'button': 'left', 'clickCount': 1, **point})
                    with contextlib.suppress(websocket.WebSocketException):
                        popup_call('Input.dispatchMouseEvent', {'type': 'mouseReleased', 'button': 'left', 'clickCount': 1, **point})

                try:
                    open_edge_toolbar()
                    record('actual-edge-toolbar-popup-opened')
                    click_edge_popup(title='关闭')
                    wait(lambda: not edge_popup_targets(), 'Edge toolbar close did not close popup', 10)
                    open_edge_toolbar()
                    record('actual-edge-toolbar-popup-close-and-reopen')
                    before = {t['task_id'] for t in tasks()}
                    click_edge_popup(text='下载')
                    wait(lambda: presenter_window(runtime=runtime), 'Edge toolbar download has no confirmation', 15)
                    assert before == {t['task_id'] for t in tasks()}, 'Download started before confirmation'
                    open_edge_toolbar()
                    wait(lambda: popup_value("document.body.textContent.includes('等待确认')"), 'Reopened Edge popup lost pending state', 10)
                    record('actual-edge-toolbar-reopen-restores-pending-confirmation')
                    click_download_confirmation(
                        accept=False,
                        runtime=runtime,
                        evidence_path=args.report.with_name('edge-toolbar-reject-confirmation.json'))
                    if edge_popup_targets():
                        click_edge_popup(title='关闭')
                        wait(lambda: not edge_popup_targets(), 'Edge popup did not close', 10)
                    open_edge_toolbar()
                    wait(lambda: popup_value("[...document.querySelectorAll('button')].some(b=>b.textContent.trim()==='下载'&&!b.disabled)"), 'Cancelled Edge popup did not restore download', 10)
                    record('actual-edge-toolbar-cancel-and-reopen-restores-download')
                    report['passed'] = True
                    return report
                finally:
                    if popup_connection:
                        popup_connection.close()
            if args.stage == 'entries':
                driver.get(fixture_url + '/index.html')
                token = uuid.uuid4().hex
                base = fixture_url + '/flower'
                url = base + '.mp4?hls-entry=' + token
                hashes={url:fixture_sha256}
                link = driver.execute_script('const a=document.createElement("a");a.href=arguments[0];a.textContent="QA download";document.body.append(a);return a', url)
                def context_command(element, label):
                    driver.execute_script('arguments[0].scrollIntoView({block:"center"})',element)
                    if label == '批量发送选中的链接':
                        driver.find_element(By.TAG_NAME, 'body').send_keys(Keys.CONTROL, 'a')
                        report['selected_text'] = driver.execute_script('return getSelection().toString()')
                        assert report['selected_text'], 'Native keyboard selection is empty'
                    if args.browser == 'edge':
                        Point = wintypes.POINT

                        Rect = wintypes.RECT

                        native_user32.GetCursorPos.argtypes = [ctypes.POINTER(Point)]
                        native_user32.GetCursorPos.restype = wintypes.BOOL
                        native_user32.WindowFromPoint.argtypes = [Point]
                        native_user32.WindowFromPoint.restype = wintypes.HWND
                        native_user32.GetAncestor.argtypes = [wintypes.HWND, wintypes.UINT]
                        native_user32.GetAncestor.restype = wintypes.HWND
                        native_user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
                        native_user32.GetWindowTextLengthW.restype = ctypes.c_int
                        native_user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
                        native_user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
                        native_user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(Rect)]
                        native_user32.IsWindowVisible.argtypes = [wintypes.HWND]
                        native_user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
                        native_user32.EnumWindows.argtypes = [ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM),
                                                              wintypes.LPARAM]
                        native_user32.EnumWindows.restype = wintypes.BOOL

                        def capture_edge_hit_failure(name, target_point, evidence):
                            from PIL import ImageGrab

                            screenshot = args.report.with_name(name + '.png')
                            ImageGrab.grab().save(screenshot)
                            top_level_windows = []

                            @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
                            def visit_window(handle, _):
                                if not native_user32.IsWindowVisible(handle):
                                    return True
                                owner = wintypes.DWORD()
                                native_user32.GetWindowThreadProcessId(handle, ctypes.byref(owner))
                                title = ctypes.create_unicode_buffer(native_user32.GetWindowTextLengthW(handle) + 1)
                                class_name = ctypes.create_unicode_buffer(256)
                                bounds = Rect()
                                native_user32.GetWindowTextW(handle, title, len(title))
                                native_user32.GetClassNameW(handle, class_name, len(class_name))
                                native_user32.GetWindowRect(handle, ctypes.byref(bounds))
                                top_level_windows.append({
                                    'hwnd': int(handle), 'pid': owner.value, 'class': class_name.value,
                                    'title': title.value,
                                    'bounds': [bounds.left, bounds.top, bounds.right, bounds.bottom],
                                })
                                return True

                            native_user32.EnumWindows(visit_window, 0)
                            diagnostic = {
                                'point': target_point, 'hit': evidence,
                                'screenshot': str(screenshot), 'visible_top_level_windows': top_level_windows,
                            }
                            report.setdefault('edge_context_hit_failures', []).append(diagnostic)
                            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
                            return diagnostic

                        windows = []
                        @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
                        def find_edge_window(hwnd, _):
                            owner = wintypes.DWORD()
                            native_user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
                            if owner.value == browser_pid and native_user32.IsWindowVisible(hwnd):
                                class_name = ctypes.create_unicode_buffer(256)
                                native_user32.GetClassNameW(hwnd, class_name, len(class_name))
                                if class_name.value == 'Chrome_WidgetWin_1':
                                    windows.append(hwnd)
                            return True
                        native_user32.EnumWindows(find_edge_window, 0)
                        assert windows, ('Native Edge browser window missing', browser_pid)
                        browser_hwnd = choose_browser_window(native_user32, windows, driver.title)
                        native_user32.GetAncestor.argtypes = [wintypes.HWND, wintypes.UINT]
                        native_user32.GetAncestor.restype = wintypes.HWND
                        native_user32.SetWindowPos(browser_hwnd, wintypes.HWND(-1), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0040)
                        focus_browser_titlebar(native_user32, browser_hwnd, browser_pid, 'Edge context-menu',
                                               args.report.with_name('edge-focus-failure.png'))
                        native_user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(Rect)]
                        native_user32.GetWindowRect.restype = wintypes.BOOL
                        browser_bounds = Rect()
                        native_user32.GetWindowRect(browser_hwnd, ctypes.byref(browser_bounds))
                        point = driver.execute_script('''const e=arguments[0],r=e.getBoundingClientRect(),hit=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);return {
                            cssRect:{left:r.left,top:r.top,right:r.right,bottom:r.bottom},
                            screen:{x:screenX,y:screenY},outerHeight,innerHeight,outerWidth,innerWidth,scale:devicePixelRatio,
                            hitTarget:hit===e||e.contains(hit),
                            x:(screenX+(outerWidth-innerWidth)/2+r.x+r.width/2)*devicePixelRatio,
                            y:(screenY+outerHeight-innerHeight-(outerWidth-innerWidth)/2+r.y+r.height/2)*devicePixelRatio
                        }''', element)
                        assert point['hitTarget'], ('Edge right-click target is occluded in page', point)
                        x, y = round(point['x']), round(point['y'])
                        assert browser_bounds.left <= x < browser_bounds.right and browser_bounds.top <= y < browser_bounds.bottom, (
                            'Edge target is outside the native browser window', point,
                            [browser_bounds.left, browser_bounds.top, browser_bounds.right, browser_bounds.bottom])
                        native_user32.SetCursorPos(x, y)
                        cursor = Point()
                        native_user32.GetCursorPos(ctypes.byref(cursor))
                        hit = native_user32.WindowFromPoint(cursor)
                        hit_pid = wintypes.DWORD()
                        if hit:
                            native_user32.GetWindowThreadProcessId(hit, ctypes.byref(hit_pid))
                        hit_root = native_user32.GetAncestor(hit, 2) if hit else None
                        hit_root_pid = wintypes.DWORD()
                        if hit_root:
                            native_user32.GetWindowThreadProcessId(hit_root, ctypes.byref(hit_root_pid))
                        target_evidence = {
                            'point': point, 'browser_bounds': [browser_bounds.left, browser_bounds.top, browser_bounds.right, browser_bounds.bottom],
                            'foreground_hwnd': int(native_user32.GetForegroundWindow()), 'browser_hwnd': int(browser_hwnd),
                            'cursor': [cursor.x, cursor.y], 'hit_hwnd': int(hit) if hit else None,
                            'hit_pid': hit_pid.value, 'hit_root_hwnd': int(hit_root) if hit_root else None,
                            'hit_root_pid': hit_root_pid.value,
                        }
                        report.setdefault('edge_context_targets', []).append(target_evidence)
                        if ((cursor.x, cursor.y) != (x, y) or hit_root != browser_hwnd
                                or hit_root_pid.value != browser_pid):
                            diagnostic = capture_edge_hit_failure('edge-context-point-hit-failure', [x, y], target_evidence)
                            raise AssertionError(diagnostic)
                        native_user32.mouse_event(8, 0, 0, 0, 0)
                        native_user32.mouse_event(16, 0, 0, 0, 0)

                        def retry_right_click():
                            focus_browser_titlebar(native_user32, browser_hwnd, browser_pid,
                                                   'Edge context-menu retry',
                                                   args.report.with_name('edge-focus-retry-failure.png'))
                            assert native_user32.SetCursorPos(x, y), ('Retry SetCursorPos failed', x, y)
                            retry_cursor = Point()
                            assert native_user32.GetCursorPos(ctypes.byref(retry_cursor))
                            retry_hit = native_user32.WindowFromPoint(retry_cursor)
                            retry_root = native_user32.GetAncestor(retry_hit, 2) if retry_hit else None
                            retry_hit_pid = wintypes.DWORD()
                            if retry_hit:
                                native_user32.GetWindowThreadProcessId(retry_hit, ctypes.byref(retry_hit_pid))
                            retry_pid = wintypes.DWORD()
                            if retry_root:
                                native_user32.GetWindowThreadProcessId(retry_root, ctypes.byref(retry_pid))
                            evidence = {
                                'point': [x, y], 'cursor': [retry_cursor.x, retry_cursor.y],
                                'foreground_hwnd': int(native_user32.GetForegroundWindow()),
                                'browser_hwnd': int(browser_hwnd),
                                'hit_hwnd': int(retry_hit) if retry_hit else None,
                                'hit_pid': retry_hit_pid.value,
                                'hit_root_hwnd': int(retry_root) if retry_root else None,
                                'hit_root_pid': retry_pid.value,
                            }
                            report.setdefault('edge_context_menu_retries', []).append(evidence)
                            assert ((retry_cursor.x, retry_cursor.y) == (x, y)
                                    and retry_root == browser_hwnd and retry_pid.value == browser_pid
                                    and native_user32.GetForegroundWindow() == browser_hwnd), \
                                capture_edge_hit_failure('edge-context-point-hit-retry-failure', [x, y], evidence)
                            native_user32.mouse_event(8, 0, 0, 0, 0)
                            native_user32.mouse_event(16, 0, 0, 0, 0)

                        edge_context_menu_command(label, browser_pid, retry_right_click, browser_hwnd)
                        return
                    ActionChains(driver).context_click(element).perform()
                    driver.set_context('chrome')
                    report.setdefault('context_menu_items', []).append(driver.execute_script("return [...document.querySelectorAll('#contentAreaContextMenu menuitem')].map(e=>({label:e.getAttribute('label'),hidden:e.hidden})).filter(e=>e.label)"))
                    item = wait(lambda: driver.execute_script("return [...document.querySelectorAll('menuitem')].find(e=>(e.getAttribute('label')||'')===arguments[0])||null", label), 'Context command missing: '+label, 10)
                    driver.execute_script('const menu=arguments[0].parentNode.parentNode;if(menu.localName==="menu")menu.openMenu(true)', item)
                    wait(lambda:item.is_displayed(),'Context item not visible: '+label,10)
                    report.setdefault('context_controls',[]).append(driver.execute_script('const e=arguments[0],r=e.getBoundingClientRect();return {label:e.getAttribute("label"),disabled:e.disabled,x:r.x,y:r.y,width:r.width,height:r.height}',item))
                    if label == '批量发送选中的链接':
                        # 与真实工具栏弹窗一样，依据浏览器提供的屏幕坐标发送系统鼠标输入。
                        x,y=driver.execute_script('const r=arguments[0].getBoundingClientRect();return [(mozInnerScreenX+r.x+r.width/2)*devicePixelRatio,(mozInnerScreenY+r.y+r.height/2)*devicePixelRatio]',item)
                        report['native_selection_control']={'x':x,'y':y,'label':label}
                        native_user32.SetCursorPos(round(x),round(y))
                        native_user32.mouse_event(2,0,0,0,0);native_user32.mouse_event(4,0,0,0,0)
                    else:
                        ActionChains(driver).click(item).perform()
                    driver.set_context('content')
                def finish(url):
                    before={t['task_id'] for t in tasks()}
                    expected={url} if isinstance(url,str) else set(url)
                    wait(lambda:presenter_window(runtime=runtime), 'Context entry has no desktop confirmation', 15)
                    from smoke_v7_presenter_visual import capture
                    capture(presenter_window(runtime=runtime), args.report.with_name(f'{args.report.stem}-confirmation-{len(report["checks"])}.png'))
                    click_download_confirmation(runtime=runtime)
                    def completed():
                        found=[t for t in tasks() if t['url'] in expected and t['task_id'] not in before]
                        created.update(t['task_id'] for t in found)
                        if found and found[-1]['status']=='failed':raise AssertionError(found[-1])
                        return found[-1] if found and found[-1]['status']=='completed' else None
                    task=wait(completed,'Context entry did not complete',60)
                    assert hashlib.sha256(Path(task['output_path']).read_bytes()).hexdigest()==hashes[task['url']]
                    record('context-entry-file-verified',task['url'])
                    return task['url']
                if args.browser == 'edge':
                    browser_pid = next(p.pid for p in psutil.process_iter(['cmdline'])
                                       if any(str(stage / 'profile') in arg for arg in p.info['cmdline'] or [])
                                       and not any(arg.startswith('--type=') for arg in p.info['cmdline'] or []))
                else:
                    browser_pid = driver.capabilities['moz:processID']
                context_command(link, '使用 HLS Downloader 下载')
                finish(url);record('actual-link-context-menu-confirmation-download-sha256')
                urls=[base+'.mp4?hls-selection='+token+'-0',base+'-copy.mp4?hls-selection='+token+'-1']
                hashes[urls[0]]=fixture_sha256
                hashes[urls[1]]=fixture_sha256
                class SelectionPage(BaseHTTPRequestHandler):
                    def log_message(self,*_):pass
                    def do_GET(self):
                        body=b'<!doctype html><html><body style="padding:40px"><h1>Selected downloads</h1></body></html>'
                        self.send_response(200);self.send_header('Content-Type','text/html; charset=utf-8');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
                selection_server=ThreadingHTTPServer(('127.0.0.1',0),SelectionPage)
                threading.Thread(target=selection_server.serve_forever,daemon=True).start()
                driver.get(f'http://127.0.0.1:{selection_server.server_port}/')
                selected=driver.execute_script('const box=document.createElement("div");for(const u of arguments[0]){const a=document.createElement("a");a.href=u;a.textContent=new URL(u).pathname.split("/").pop();box.append(a,document.createElement("br"))}document.body.append(box);const r=document.createRange();r.selectNodeContents(box);getSelection().removeAllRanges();getSelection().addRange(r);return box.querySelector("a")',urls)
                context_command(selected,'批量发送选中的链接')
                wait(lambda: driver.execute_script('return document.querySelector("hls-downloader-media-panel")?.shadowRoot?.querySelectorAll(".item").length')==2,'Selection panel did not contain exactly two selected links',15)
                assert shadow('.pin').text=='固定', 'Selection test must not rely on pinned mode'
                driver.execute_script('window.__hlsQaClicks=[];document.querySelector("hls-downloader-media-panel").shadowRoot.addEventListener("click",e=>window.__hlsQaClicks.push({text:e.target.textContent,trusted:e.isTrusted,row:e.target.closest(".item")?.querySelector(".resource-url")?.textContent}),true)')
                driver.execute_script('window.addEventListener("mousemove",e=>window.__hlsQaPointer={x:e.clientX,y:e.clientY},true)')
                windows=[]
                @ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
                def locate_browser(hwnd,_):
                    owner=wintypes.DWORD()
                    native_user32.GetWindowThreadProcessId(hwnd,ctypes.byref(owner))
                    if owner.value==browser_pid and native_user32.IsWindowVisible(hwnd):windows.append(hwnd)
                    return True
                native_user32.EnumWindows(locate_browser,0)
                assert windows,'Test browser window missing'
                received=[]
                for wanted in urls:
                    # 显示地址隐藏查询参数；按真实文件名定位，再以任务原始地址核验链接。
                    filename=wanted.split('?',1)[0].rsplit('/',1)[1]
                    button=driver.execute_script('const s=document.querySelector("hls-downloader-media-panel").shadowRoot;return [...s.querySelectorAll(".item")].find(e=>e.querySelector(".resource-url")?.textContent.endsWith(arguments[0]))?.querySelector("button.download")',filename)
                    assert button, 'Selected link action missing'
                    assert button.is_displayed() and button.is_enabled(),filename
                    driver.execute_script('arguments[0].scrollIntoView({block:"center",inline:"nearest"})', button)
                    time.sleep(.2)
                    report.setdefault('selection_targets', []).append(driver.execute_script('const b=arguments[0],s=document.querySelector("hls-downloader-media-panel").shadowRoot,r=b.getBoundingClientRect();return {wanted:arguments[1],rows:[...s.querySelectorAll(".item")].map(e=>({name:e.querySelector(".name")?.textContent,url:e.querySelector(".resource-url")?.textContent})),target:b.closest(".item")?.querySelector(".resource-url")?.textContent,bounds:[r.x,r.y,r.width,r.height],hit:s.elementFromPoint(r.x+r.width/2,r.y+r.height/2)?.outerHTML?.slice(0,500)}', button, wanted))
                    native_user32.SetForegroundWindow(windows[0])
                    before_click=driver.execute_script('return window.__hlsQaClicks.length')
                    ActionChains(driver).move_to_element(button).pause(.1).perform()
                    report.setdefault('selection_after_mouse_move', []).append(driver.execute_script('const b=arguments[0],s=document.querySelector("hls-downloader-media-panel").shadowRoot,r=b.getBoundingClientRect(),w=s.querySelector(".wrap").getBoundingClientRect(),p=window.__hlsQaPointer;return {wanted:arguments[1],button:[r.x,r.y,r.width,r.height],wrap:[w.x,w.y,w.width,w.height],hit:s.elementFromPoint(r.x+r.width/2,r.y+r.height/2)?.outerHTML?.slice(0,500),pointer:p,pointer_hit:p?s.elementFromPoint(p.x,p.y)?.outerHTML?.slice(0,500):null,scroll_y:scrollY}', button, wanted))
                    ActionChains(driver).click().perform()
                    wait(lambda:driver.execute_script('return window.__hlsQaClicks.length')>before_click,'Native page click was not delivered',5)
                    report['selection_clicks']=driver.execute_script('return window.__hlsQaClicks')
                    assert report['selection_clicks'][-1]['row'].endswith(filename), report['selection_clicks'][-1]
                    assert report['selection_clicks'][-1]['text'] == '下载', report['selection_clicks'][-1]
                    received.append(finish(wanted))
                    assert shadow('.wrap').is_displayed(), 'Selected links disappeared after desktop confirmation'
                assert set(received)==set(urls) and len(received)==len(urls),received
                record('actual-selection-context-menu-two-links-confirmation-download-sha256')
                ActionChains(driver).click(shadow('.close')).perform()
                wait(lambda:not shadow('.wrap').is_displayed(),'Selected links panel did not close explicitly',5)
                record('selected-links-stay-open-without-pin-and-close-explicitly')
                report['passed']=True
                return report
            if args.stage == 'errors':
                before = {t['task_id'] for t in tasks()}
                driver.get(fixture_url + '/index.html')
                link = driver.execute_script('const a=document.createElement("a");a.href=arguments[0];a.textContent="QA download link";a.style="position:fixed;top:10px;left:10px;z-index:2147483647;background:white;padding:20px";document.body.append(a);return a',fixture_url+'/flower.mp4')
                ActionChains(driver).context_click(link).perform()
                if args.browser == 'edge':
                    browser_pid = next(p.pid for p in psutil.process_iter(['cmdline'])
                                       if any(str(stage / 'profile') in arg for arg in p.info['cmdline'] or [])
                                       and not any(arg.startswith('--type=') for arg in p.info['cmdline'] or []))
                    edge_windows = []
                    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
                    def find_error_edge_window(hwnd, _):
                        owner = wintypes.DWORD()
                        native_user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
                        class_name = ctypes.create_unicode_buffer(256)
                        native_user32.GetClassNameW(hwnd, class_name, len(class_name))
                        if (owner.value == browser_pid and native_user32.IsWindowVisible(hwnd)
                                and class_name.value == 'Chrome_WidgetWin_1'):
                            edge_windows.append(hwnd)
                        return True
                    native_user32.EnumWindows(find_error_edge_window, 0)
                    error_browser_hwnd = choose_browser_window(native_user32, edge_windows, driver.title)
                    edge_context_menu_command('使用 HLS Downloader 下载', browser_pid,
                                              browser_hwnd=error_browser_hwnd)
                else:
                    driver.set_context('chrome')
                    item = wait(lambda: driver.execute_script("return [...document.querySelectorAll('menuitem')].find(e=>(e.getAttribute('label')||'')==='使用 HLS Downloader 下载')||null"), 'Real extension context menu missing', 10)
                    driver.execute_script('const menu=arguments[0].parentNode.parentNode;if(menu.localName==="menu")menu.openMenu(true)', item)
                    ActionChains(driver).click(item).perform()
                    driver.set_context('content')
                def page_error():
                    return driver.execute_script('const r=document.querySelector("hls-downloader-media-panel")?.shadowRoot?.querySelector(".result.error");return r&&!r.hidden?r.textContent:null')
                error = wait(page_error, 'Context download failure remained invisible', 40)
                assert before == {t['task_id'] for t in tasks()}, 'Rejected connection created a task'
                record('real-context-menu-failure-visible-on-page', error)
                driver.switch_to.new_window('tab')
                driver.get(popup_url)
                wait(lambda: driver.find_elements(By.CSS_SELECTOR,'.send-error:not([hidden])'), 'Context error lost when opening popup', 10)
                assert error in driver.find_element(By.CSS_SELECTOR,'.send-error').text
                record('context-menu-error-retained-in-popup')
                report['passed'] = True
                return report
            if args.stage == 'toolbar':
                driver.get(fixture_url + '/index.html')
                for frame in driver.find_elements(By.TAG_NAME, 'iframe'):
                    driver.switch_to.frame(frame)
                    if driver.find_elements(By.TAG_NAME, 'video'): break
                    driver.switch_to.default_content()
                driver.execute_script('const v=document.querySelector("video");if(v){v.muted=true;v.loop=true;v.play().catch(()=>{})}')
                wait(lambda: driver.execute_script('return Boolean(document.querySelector("hls-downloader-media-panel")?.shadowRoot?.querySelector(".video-download:not(.identifying)"))'), 'Public video not recognized', 25)
                driver.switch_to.default_content()
                browser_title = driver.title
                driver.set_context('chrome')
                user32 = ctypes.WinDLL('user32')
                windows = []
                def visible_browser(hwnd, _):
                    pid = wintypes.DWORD()
                    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
                    if pid.value == driver.capabilities['moz:processID'] and user32.IsWindowVisible(hwnd):
                        class_name = ctypes.create_unicode_buffer(256)
                        user32.GetClassNameW(hwnd, class_name, len(class_name))
                        if class_name.value == 'MozillaWindowClass':
                            windows.append(hwnd)
                    return True
                enum_callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)(visible_browser)
                user32.EnumWindows(enum_callback, 0)
                assert windows, 'Native Firefox window missing'
                browser_hwnd = choose_browser_window(user32, windows, browser_title)
                user32.SetWindowPos(browser_hwnd, wintypes.HWND(-1), 0,0,0,0,0x0001|0x0002|0x0040)
                focus_browser_titlebar(user32, browser_hwnd, driver.capabilities['moz:processID'], 'Firefox toolbar',
                                       args.report.with_name('firefox-focus-failure.png'))
                # 浏览器原生工具栏控件，不能拿独立 popup.html 标签页替代。
                widget_id = re.sub(r'[^a-z0-9_-]', '_', addon_id.lower()) + '-browser-action'
                driver.execute_script("ChromeUtils.importESModule('moz-src:///browser/components/customizableui/CustomizableUI.sys.mjs').CustomizableUI.addWidgetToArea(arguments[0], 'nav-bar')", widget_id)
                widget = wait(lambda: driver.execute_script('return document.getElementById(arguments[0])', widget_id), 'Toolbar widget not pinned', 10)
                if not widget:
                    widget = driver.execute_script("return [...document.querySelectorAll('toolbarbutton')].find(e=>(e.getAttribute('label')||'').includes('HLS Downloader'))")
                assert widget, 'Native browser toolbar widget missing'
                ActionChains(driver).click(widget).perform()
                panel_browser = wait(lambda: driver.execute_script("return [...document.querySelectorAll('panel browser')].find(b => b.getAttribute('webextension-view-type')==='popup' && b.closest('panel')?.state==='open') || null"), 'Native popup browser missing', 10)
                report['native_popup_attributes'] = driver.execute_script('return [...arguments[0].attributes].map(a=>[a.name,a.value])', panel_browser)
                driver.execute_script("window.__hlsQaPopupHidden=false;arguments[0].closest('panel').addEventListener('popuphidden',()=>window.__hlsQaPopupHidden=true,{once:true})",panel_browser)
                state = wait(lambda: (s if (s:=toolbar_state()).get('ready')=='ready' else None), 'Toolbar popup not ready', 15)
                assert 'HLS Downloader' in state['text']
                record('actual-toolbar-popup-opened', state)
                def click_toolbar_button(text=None, title=None):
                    def visible_target():
                        current = toolbar_state({'text': text, 'title': title})
                        button = next((b for b in current.get('buttons', []) if (b['text']==text if text else b['title']==title)), None)
                        return current if (current.get('ready')=='ready' and button and button['visible']) else None
                    state = wait(visible_target, 'Toolbar target did not settle inside the popup content viewport', 10)
                    b = next(b for b in state['buttons'] if (b['text']==text if text else b['title']==title))
                    assert not b['disabled'], b
                    from PIL import ImageGrab

                    class Point(ctypes.Structure):
                        _fields_ = [('x', wintypes.LONG), ('y', wintypes.LONG)]

                    user32.GetDpiForWindow.argtypes = [wintypes.HWND]
                    user32.GetDpiForWindow.restype = wintypes.UINT
                    user32.GetCursorPos.argtypes = [ctypes.POINTER(Point)]
                    user32.GetCursorPos.restype = wintypes.BOOL
                    user32.WindowFromPoint.argtypes = [Point]
                    user32.WindowFromPoint.restype = wintypes.HWND
                    user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
                    user32.GetWindowRect.restype = wintypes.BOOL
                    user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
                    user32.GetWindowTextLengthW.restype = ctypes.c_int
                    user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
                    user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
                    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
                    user32.GetWindowThreadProcessId.restype = wintypes.DWORD
                    user32.GetForegroundWindow.restype = wintypes.HWND

                    def window_details(hwnd):
                        rect = wintypes.RECT()
                        user32.GetWindowRect(hwnd, ctypes.byref(rect))
                        owner = wintypes.DWORD()
                        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
                        length = user32.GetWindowTextLengthW(hwnd)
                        title_buffer = ctypes.create_unicode_buffer(length + 1)
                        class_buffer = ctypes.create_unicode_buffer(256)
                        user32.GetWindowTextW(hwnd, title_buffer, length + 1)
                        user32.GetClassNameW(hwnd, class_buffer, len(class_buffer))
                        return {
                            'hwnd': int(hwnd), 'pid': owner.value, 'title': title_buffer.value,
                            'class': class_buffer.value,
                            'bounds': [rect.left, rect.top, rect.right, rect.bottom],
                        }

                    dpi_scale = user32.GetDpiForWindow(browser_hwnd) / 96
                    assert abs(dpi_scale-state['scale']) < .05, ('Firefox CSS/native DPI mismatch', dpi_scale, state['scale'])
                    target = (round(b['x']*dpi_scale), round(b['y']*dpi_scale))
                    user32.SetCursorPos(*target)
                    cursor = Point()
                    user32.GetCursorPos(ctypes.byref(cursor))
                    hit = user32.WindowFromPoint(cursor)
                    hit_details = window_details(hit) if hit else None
                    evidence = {
                        'button': {'text': b['text'], 'title': b['title'], 'rect': b['rect']},
                        'content_viewport': state['viewport'], 'reported_css_center': [b['x'], b['y']],
                        'reported_scale': state['scale'], 'native_dpi_scale': dpi_scale,
                        'target_screen_point': list(target), 'actual_cursor': [cursor.x, cursor.y],
                        'foreground_window': window_details(user32.GetForegroundWindow()),
                        'foreground_pid': window_details(user32.GetForegroundWindow())['pid'],
                        'window_from_point': hit_details,
                        'browser_window': window_details(browser_hwnd),
                    }
                    bounds = hit_details['bounds'] if hit_details else (0, 0, 0, 0)
                    hit_is_popup = bool(hit_details and hit_details['pid'] == driver.capabilities['moz:processID']
                                        and hit_details['class'] == 'MozillaDropShadowWindowClass'
                                        and bounds[0] <= cursor.x < bounds[2] and bounds[1] <= cursor.y < bounds[3])
                    evidence['native_popup_hit'] = hit_is_popup
                    report.setdefault('toolbar_button_clicks', []).append(evidence)
                    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
                    assert (cursor.x, cursor.y) == target and hit_is_popup, (
                        'Toolbar target is outside the native Firefox popup bounds', evidence)
                    assert evidence['foreground_pid'] == driver.capabilities['moz:processID'], (
                        'Firefox native popup is not foreground', evidence)
                    before_image = args.report.with_name(f'{args.report.stem}-toolbar-{text or title}-before.png')
                    ImageGrab.grab().save(before_image)
                    evidence['screenshot_before'] = str(before_image)
                    user32.mouse_event(2,0,0,0,0);user32.mouse_event(4,0,0,0,0)
                    time.sleep(.1)
                    after_image = args.report.with_name(f'{args.report.stem}-toolbar-{text or title}-after.png')
                    ImageGrab.grab().save(after_image)
                    evidence['screenshot_after'] = str(after_image)
                    if title == '关闭':
                        report.setdefault('toolbar_close_clicks', []).append(evidence)
                    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
                click_toolbar_button(title='关闭')
                wait(lambda: toolbar_state().get('ready') == 'closed', 'Toolbar close did not close popup', 5)
                widget = driver.execute_script('return document.getElementById(arguments[0])',widget_id)
                ActionChains(driver).click(widget).perform()
                wait(lambda: toolbar_state().get('ready')=='ready', 'Toolbar reopen failed', 10)
                record('actual-toolbar-popup-close-and-reopen')
                click_toolbar_button(text='下载')
                wait(lambda: presenter_window(runtime=runtime), 'Toolbar download has no confirmation', 12)
                assert initial <= {t['task_id'] for t in tasks()}
                widget = driver.execute_script('return document.getElementById(arguments[0])',widget_id)
                ActionChains(driver).click(widget).perform()
                state = wait(lambda: (s if '等待确认' in (s:=toolbar_state()).get('text','') else None), 'Reopened popup lost pending confirmation', 10)
                record('actual-toolbar-reopen-restores-pending-confirmation', '等待确认' in state['text'])
                click_download_confirmation(accept=False, runtime=runtime)
                widget = driver.execute_script('return document.getElementById(arguments[0])',widget_id)
                ActionChains(driver).click(widget).perform()
                def retryable_toolbar():
                    state = toolbar_state()
                    return state if any(b['text']=='下载' and not b['disabled'] for b in state.get('buttons',[])) else None
                state = wait(retryable_toolbar, 'Cancelled toolbar download did not become retryable', 10)
                record('actual-toolbar-cancel-and-reopen-restores-download')
                report['passed'] = True
                return report
            for kind, page in PUBLIC:
                if args.kind and kind != args.kind: continue
                before = {t['task_id'] for t in tasks()}
                driver.get(page)
                video = wait(lambda: next(iter(driver.find_elements(By.TAG_NAME, 'video')), None), 'Public player video missing', 20)
                driver.execute_script('arguments[0].muted=true;arguments[0].play().catch(()=>{})', video)
                wait(lambda: driver.execute_script('return Boolean(document.querySelector("hls-downloader-media-panel")?.shadowRoot?.querySelector(".video-download:not(.identifying)"))'), f'{kind} resource not associated', 40)
                wait(lambda: _open_first_media_actions(driver), 'Media panel did not open', 40)
                wait(lambda: shadow('.quality-select'), 'Rendition selector missing', 30)
                # 选最低清晰度以完成整个公开点播文件，仍检查实际产物而非截断流。
                options = driver.execute_script('return [...arguments[0].options].map(o=>({value:o.value,text:o.textContent}))', shadow('.quality-select'))
                def rendition_order(option):
                    height = re.search(r'(\d+)p', option['text'])
                    bitrate = re.search(r'([\d.]+) Mbps', option['text'])
                    return (int(height.group(1)) if height else 100000, float(bitrate.group(1)) if bitrate else 100000)
                chosen = min(options[1:], key=rendition_order)
                expected_height = int(re.search(r'(\d+)p', chosen['text']).group(1))
                # 清单初次补齐音轨/码率时允许控件换代，重新定位同一选项。
                for attempt in range(3):
                    try:
                        Select(shadow('.quality-select')).select_by_value(chosen['value'])
                        break
                    except StaleElementReferenceException:
                        if attempt == 2: raise
                assert shadow('.quality-select').get_attribute('value') == chosen['value']
                button = shadow('.item-actions button.download')
                ActionChains(driver).click(button).perform()
                wait(lambda: presenter_window(runtime=runtime), 'Desktop confirmation missing', 12)
                assert before == {t['task_id'] for t in tasks()}, 'Task started before desktop confirmation'
                click_download_confirmation(runtime=runtime)
                def current():
                    found = [t for t in tasks() if t['task_id'] not in before and t['resource_kind'] == kind]
                    created.update(t['task_id'] for t in found)
                    if found and found[-1]['status'] == 'failed': raise AssertionError(found[-1])
                    return found[-1] if found and found[-1]['status'] == 'completed' else None
                task = wait(current, f'{kind} public download did not finish', 600)
                path = Path(task['output_path'])
                probe = json.loads(subprocess.check_output([str(runtime/'app/resources/ffprobe.exe'), '-v', 'error', '-show_streams', '-show_format', '-of', 'json', str(path)], timeout=30))
                video_stream = next(s for s in probe['streams'] if s['codec_type'] == 'video')
                assert video_stream['height'] == expected_height, (chosen['text'], video_stream)
                assert any(s['codec_type'] == 'audio' for s in probe['streams']), 'Output has no audio'
                subprocess.run([str(runtime/'app/resources/ffmpeg.exe'), '-v', 'error', '-xerror', '-i', str(path), '-f', 'null', '-'], check=True, timeout=180, capture_output=True)
                record(f'{kind}-webpage-rendition-confirmation-file-decoded', {'height': expected_height, 'audio': True, 'duration': probe['format']['duration'], 'bytes': path.stat().st_size})
                core({'type': 'command', 'request_id': 2, 'command': {'kind': 'task_action', 'task_id': task['task_id'], 'action': 'delete_files'}})
                created.discard(task['task_id'])
            report['passed'] = True
        except Exception as error:
            report['error'] = str(error)
            report['traceback'] = traceback.format_exc()
            if driver and args.stage=='entries':
                with contextlib.suppress(Exception):
                    if args.browser == 'firefox':
                        driver.set_context('content')
                    report['last_context_tasks'] = [{key:task.get(key) for key in ['task_id','url','status','error_message']} for task in tasks() if task['task_id'] not in initial]
                    report['last_context_handoffs'] = [json.loads(item) for item in core({'type':'load_handoffs','request_id':3})['items'] if any(marker in json.loads(item).get('offer',{}).get('url','') for marker in ['hls-entry='+token,'hls-selection='+token+'-'])]
                    report['page_error']=driver.execute_script('return document.querySelector("hls-downloader-media-panel")?.shadowRoot?.querySelector(".result")?.textContent')
                    driver.save_screenshot(str(args.report.with_name(args.report.stem+'-page-failure.png')))
                    driver.switch_to.new_window('tab');driver.get(popup_url)
                    report['connection']=extension_message({'type':'ping'})
                    report['popup_error']=driver.find_element(By.TAG_NAME,'body').text
            if driver:
                with contextlib.suppress(Exception): driver.save_screenshot(str(args.report.with_suffix('.png')))
        finally:
            if driver:
                if args.stage == 'toolbar' and 'browser_hwnd' in locals():
                    with contextlib.suppress(Exception): user32.SetWindowPos(browser_hwnd,wintypes.HWND(-2),0,0,0,0,0x0001|0x0002|0x0010)
                with contextlib.suppress(Exception): driver.quit()
            if selection_server:
                selection_server.shutdown();selection_server.server_close()
            if video_server:
                video_server.shutdown();video_server.server_close()
            if args.stage=='entries' and 'token' in locals():
                markers=['hls-entry='+token,'hls-selection='+token+'-']
                created.update(t['task_id'] for t in tasks() if any(m in t['url'] for m in markers))
            # 失败的工具栏验收也会留下待确认请求，必须收回本轮服务器的交接，避免遮挡下一项。
            fixture_prefixes = [fixture_url + '/'] if 'fixture_url' in locals() else []
            if selection_server:
                fixture_prefixes.append(f'http://127.0.0.1:{selection_server.server_port}/')
            for encoded in core({'type':'load_handoffs','request_id':3})['items']:
                handoff=json.loads(encoded)
                if handoff.get('status')=='pending' and any(handoff.get('offer',{}).get('url','').startswith(prefix) for prefix in fixture_prefixes):
                    core({'type':'command','request_id':4,'command':{'kind':'reject_handoff','handoff_id':handoff['id'],'suppress_site_kind':False}})
            for task_id in created:
                core({'type': 'command', 'request_id': 2, 'command': {'kind': 'task_action', 'task_id': task_id, 'action': 'delete_files'}})
            report['user_tasks_preserved'] = initial <= {t['task_id'] for t in tasks()}
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
    return report


if __name__ == '__main__':
    sys.stdout.reconfigure(encoding='utf-8')
    parser = argparse.ArgumentParser()
    parser.add_argument('--browser', choices=['firefox', 'edge'], required=True)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--extension', type=Path, required=True)
    parser.add_argument('--driver', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--stage', choices=['media', 'toolbar', 'errors', 'entries'], default='media')
    parser.add_argument('--kind', choices=['hls', 'dash'])
    result = run(parser.parse_args())
    print(json.dumps({k:v for k,v in result.items() if k != 'extension_sha256'}, ensure_ascii=False, indent=2))
    raise SystemExit(0 if result['passed'] else 1)
