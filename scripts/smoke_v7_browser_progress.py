"""真实浏览器交接、媒体传输 HUD、清晰度/音轨及识别超时验收。"""
import argparse
import base64
import ctypes
from ctypes import wintypes
import functools
import hashlib
import http.server
import json
import subprocess
import tempfile
import threading
import time
import zipfile
from pathlib import Path

from PIL import Image, ImageChops
from selenium import webdriver
from selenium.webdriver import ActionChains
from selenium.webdriver.support.ui import Select
from selenium.webdriver.firefox.options import Options as FirefoxOptions
from selenium.webdriver.firefox.service import Service as FirefoxService
from selenium.webdriver.edge.options import Options as EdgeOptions
from selenium.webdriver.edge.service import Service as EdgeService
from smoke_extension_media import _make_media, _find_edge, _find_firefox, _fit_browser_window
from smoke_v7_browser_download_confirmation import presenter_window, click_download_confirmation
from smoke_v7_portable_app import Pipe, wait
from smoke_v7_presenter_visual import user32, capture


def core(request):
    pipe = Pipe()
    try:
        return pipe.request(request)
    finally:
        pipe.close()


def command(value):
    return core({'type': 'command', 'request_id': 1, 'command': value})


def run(args):
    root = Path(__file__).resolve().parents[1]
    args.report.parent.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'browser': args.browser, 'media': args.media, 'authenticated': args.authenticated,
              'cross_iframe': args.cross_iframe, 'redirect_init': args.redirect_init,
              'authenticated_headers': args.authenticated_headers, 'opaque_manifest': args.opaque_manifest, 'checks': [],
              'test_script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'extension_sha256': {str(p.relative_to(args.extension)): hashlib.sha256(p.read_bytes()).hexdigest()
                                   for p in sorted(args.extension.rglob('*')) if p.is_file()},
              'installed_sha256': {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                                   for p in args.ffmpeg.parent.glob('HLSDownloader*.exe')}}
    created = set()
    driver = None
    server = None
    frame_server = None
    temporary = None
    origin = None
    media_url = None
    original_tasks = core({'type': 'snapshot', 'request_id': 1})['tasks']
    assert not any(t['status'] in ('downloading', 'recording', 'merging', 'checking') for t in original_tasks), 'User download active'
    initial_ids = {t['task_id'] for t in original_tasks}
    workbench = user32.FindWindowW(None, 'HLS Downloader')
    was_visible = bool(workbench and user32.IsWindowVisible(workbench))
    if workbench:
        user32.PostMessageW(workbench, 0x0010, 0, 0)
        wait(lambda: not user32.IsWindowVisible(workbench), 'Workbench did not hide', 10)

    def record(name, evidence):
        report['checks'].append({'name': name, 'evidence': evidence})
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
        print(name, flush=True)

    try:
        temporary = tempfile.TemporaryDirectory(prefix="browser-progress-", dir=root / ".tool-cache/test-tmp")
        stage = Path(temporary.name)
        site = stage / 'site'
        site.mkdir()
        _make_media(site, str(args.ffmpeg))
        if args.media == 'dash':
            # 两档真实视频及独立音轨；不能使用识别测试中仅修改清单元数据的高清轨。
            subprocess.run([str(args.ffmpeg), '-v', 'error', '-y', '-i', str(site / 'stream.mp4'),
                            '-map', '0:v:0', '-map', '0:v:0', '-map', '0:a:0', '-t', '4',
                            '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-g', '30',
                            '-filter:v:0', 'scale=320:180', '-b:v:0', '200k', '-b:v:1', '600k',
                            '-c:a', 'aac', '-b:a', '64k', '-f', 'dash', '-seg_duration', '1',
                            '-use_template', '1', '-use_timeline', '1',
                            '-adaptation_sets', 'id=0,streams=v id=1,streams=a',
                            '-init_seg_name', 'init-$RepresentationID$.m4s',
                            '-media_seg_name', 'chunk-$RepresentationID$-$Number%05d$.m4s',
                            str((site / 'dash/manifest.mpd').resolve())], cwd=site / 'dash', check=True, timeout=60)
            report['fixture_manifest'] = (site / 'dash/manifest.mpd').read_text(encoding='utf-8')
        if args.authenticated and args.media != 'file':
            manifest = site / ('dash/manifest.mpd' if args.media == 'dash' else 'hls/index.m3u8')
            text = manifest.read_text(encoding='utf-8')
            if args.media == 'hls':
                text = text.replace('URI="init.mp4"', 'URI="/init/hls/init.mp4"')
                text = '\n'.join('/chunks/hls/' + line if line.startswith('segment-') else line for line in text.splitlines()) + '\n'
            else:
                text = text.replace('initialization="init-', 'initialization="/init/dash/init-')
                text = text.replace('media="chunk-', 'media="/chunks/dash/chunk-')
            manifest.write_text(text, encoding='utf-8')
        if args.media == 'file':
            (site / 'index.html').write_text('<video controls autoplay muted loop width="640" height="360" src="/chunks/download?token=browser-smoke"></video>', encoding='utf-8')
        if args.authenticated_headers:
            assert args.authenticated and not args.redirect_init
            page = (site / 'index.html').read_text(encoding='utf-8')
            page = page.replace('const mode =', '''const originalPageFetch = window.fetch;
window.fetch = (url, init = {}) => {
  const path = new URL(url, location.href).pathname;
  const token = path.startsWith('/init/') ? 'init' : path.startsWith('/chunks/') ? 'chunks' : 'manifest';
  const headers = new Headers(init.headers); headers.set('Authorization', 'Bearer ' + token);
  headers.set('X-Playback-Token', token);
  return originalPageFetch(url, {...init, headers});
};
const mode =''')
            (site / 'index.html').write_text(page, encoding='utf-8')
        if args.opaque_manifest:
            assert args.authenticated and args.media == 'hls' and not args.cross_iframe
            page = (site / 'index.html').read_text(encoding='utf-8')
            page = page.replace('/hls/index.m3u8?token=browser-smoke', '/opaque?token=browser-smoke')
            (site / 'index.html').write_text(page, encoding='utf-8')
        # 无网络来源的 Blob 可稳定制造“播放器在播放但资源无法关联”。
        encoded = base64.b64encode((site / 'stream.mp4').read_bytes()).decode('ascii')
        (site / 'unresolved.html').write_text('''<video width="640" height="360" muted autoplay loop></video><script>
const v=document.querySelector('video');
v.src=URL.createObjectURL(new Blob([Uint8Array.from(atob(''' + json.dumps(encoded) + '''),c=>c.charCodeAt(0))],{type:'video/mp4'}));v.play();
</script>''', encoding='utf-8')

        transfer_observation_ready = threading.Event()

        class Handler(http.server.SimpleHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def end_headers(self):
                if self.headers.get('Origin') == frame_origin:
                    self.send_header('Access-Control-Allow-Origin', frame_origin)
                    self.send_header('Access-Control-Allow-Credentials', 'true')
                    self.send_header('Access-Control-Allow-Headers', 'Authorization, X-Playback-Token')
                if self.path.startswith('/index.html'):
                    self.send_header('Referrer-Policy', 'unsafe-url')
                if args.authenticated and self.path.startswith(('/index.html', '/outer.html')):
                    for value, path in [('page', '/index.html'), ('manifest', '/hls' if args.media == 'hls' else '/dash'),
                                        ('manifest', '/opaque'), ('init', '/init'), ('chunks', '/chunks')]:
                        self.send_header('Set-Cookie', f'hlsqa={value}; Path={path}; HttpOnly; SameSite=Lax')
                super().end_headers()

            def translate_path(self, path):
                if path.startswith('/chunks/download?'):
                    path = '/stream.mp4'
                if path.startswith('/opaque?'):
                    path = '/hls/index.m3u8'
                return super().translate_path(path.replace('/chunks/', '/', 1).replace('/init/', '/', 1))

            def guess_type(self, path):
                if args.opaque_manifest and path.endswith('index.m3u8'):
                    return 'application/octet-stream'
                return super().guess_type(path)

            def do_OPTIONS(self):
                self.send_response(204); self.end_headers()

            def do_GET(self):
                entry = {'path': self.path, 'core': self.headers.get('Accept-Encoding') == 'identity',
                         'cookie': self.headers.get('Cookie', ''), 'referer': self.headers.get('Referer', ''),
                         'origin': self.headers.get('Origin', ''), 'authorization': self.headers.get('Authorization', ''),
                         'playback_token': self.headers.get('X-Playback-Token', '')}
                report.setdefault('http_requests', []).append(entry)
                if args.authenticated and not self.path.startswith(('/index.html', '/unresolved.html')):
                    expected = next((value for prefix, value in [('/hls/', 'manifest'), ('/dash/', 'manifest'),
                                    ('/opaque?', 'manifest'), ('/init/', 'init'), ('/chunks/', 'chunks')] if self.path.startswith(prefix)), None)
                    if expected and (entry['cookie'] != f'hlsqa={expected}' or not entry['referer'].startswith(frame_origin + '/index.html')):
                        entry['rejected'] = True
                        self.send_error(403, 'Wrong path cookie or source page'); return
                    if args.authenticated_headers and expected and (entry['authorization'] != f'Bearer {expected}' or entry['playback_token'] != expected):
                        entry['rejected'] = True
                        self.send_error(403, 'Wrong path authorization'); return
                if args.redirect_init and self.path.startswith('/init/'):
                    self.send_response(302)
                    self.send_header('Location', self.path.replace('/init/', '/chunks/', 1))
                    self.end_headers(); return
                if entry['core'] and ('segment-' in self.path or '/dash/chunk-' in self.path):
                    # 先让确认夹具完成截图和窗口等待，再开始传输，避免快下载抢在采样前结束。
                    if not transfer_observation_ready.wait(60):
                        self.send_error(504, 'Progress observer did not start'); return
                # 浏览器正常播放；Core 的分片逐个错开，使传输中进度可观察。
                if 'segment-' in self.path and self.headers.get('Accept-Encoding') == 'identity':
                    index = int(self.path.split('segment-')[1].split('.')[0])
                    time.sleep(6 + index * 4)
                if '/dash/chunk-' in self.path and self.headers.get('Accept-Encoding') == 'identity':
                    time.sleep(1.2)
                return super().do_GET()

            def copyfile(self, source, outputfile):
                if self.headers.get('Accept-Encoding') == 'identity' and ('segment-' in self.path or '/dash/chunk-' in self.path):
                    while block := source.read(1024):
                        outputfile.write(block)
                        outputfile.flush()
                        time.sleep(.1)
                else:
                    super().copyfile(source, outputfile)

            def send_error(self, code, message=None, explain=None):
                report.setdefault('http_errors', []).append({'path': self.path, 'status': code})
                return super().send_error(code, message, explain)

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Handler, directory=str(site)))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        origin = f'http://127.0.0.1:{server.server_port}'
        frame_origin = origin
        if args.cross_iframe:
            assert args.authenticated and args.media == 'hls', 'Cross iframe uses authenticated HLS'
            frame_server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Handler, directory=str(site)))
            threading.Thread(target=frame_server.serve_forever, daemon=True).start()
            frame_origin = f'http://127.0.0.1:{frame_server.server_port}'
            page = (site / 'index.html').read_text(encoding='utf-8')
            page = page.replace("fetch('/hls/index.m3u8?token=browser-smoke')",
                                "fetch('" + origin + "/hls/index.m3u8?token=browser-smoke',{credentials:'include'})")
            # 分片地址是跨域的，仍由播放器自身携带浏览器凭据。
            page = page.replace('fetch(url)', "fetch(url,{credentials:'include'})")
            (site / 'index.html').write_text(page, encoding='utf-8')
            (site / 'outer.html').write_text(f'<iframe width="900" height="600" src="{frame_origin}/index.html?mode=hls-mse"></iframe>', encoding='utf-8')
        extension = args.extension.resolve()
        if args.browser == 'firefox':
            options = FirefoxOptions()
            options.binary_location = str(_find_firefox())
            options.set_preference('media.autoplay.default', 0)
            options.set_preference('media.autoplay.blocking_policy', 0)
            options.set_preference('browser.shell.checkDefaultBrowser', False)
            options.set_preference('browser.aboutwelcome.enabled', False)
            options.set_preference('browser.newtabpage.activity-stream.asrouter.providers.onboarding', '{"id":"onboarding","type":"local","localProvider":"OnboardingMessageProvider","enabled":false}')
            options.set_preference('browser.newtabpage.activity-stream.asrouter.providers.messaging-experiments', '{"id":"messaging-experiments","type":"remote-experiments","enabled":false}')
            driver = webdriver.Firefox(options=options, service=FirefoxService(str(args.driver),
                                       service_args=['--profile-root', str(stage), '--allow-system-access', '--log', 'debug'],
                                       log_output=str(args.report.with_name(args.report.stem + '-webdriver.log'))))
            addon = stage / 'extension.xpi'
            with zipfile.ZipFile(addon, 'w', zipfile.ZIP_DEFLATED) as archive:
                for path in extension.rglob('*'):
                    if path.is_file():
                        archive.write(path, path.relative_to(extension).as_posix())
            driver.install_addon(str(addon), temporary=True)
        else:
            options = EdgeOptions()
            options.binary_location = str(_find_edge())
            for flag in [f'--user-data-dir={stage / "profile"}', '--disable-features=DisableLoadExtensionCommandLineSwitch',
                         f'--load-extension={extension}', '--no-first-run', '--autoplay-policy=no-user-gesture-required']:
                options.add_argument(flag)
            driver = webdriver.Edge(options=options, service=EdgeService(str(args.driver)))
        if args.authenticated:
            popup_url = 'chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/popup.html'
            if args.browser == 'firefox':
                driver.set_context('chrome')
                uuids = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
                addon_id = json.loads((extension / 'manifest.json').read_text(encoding='utf-8'))['browser_specific_settings']['gecko']['id']
                driver.set_context('content')
                popup_url = f'moz-extension://{uuids[addon_id]}/popup.html'
            driver.get(popup_url)
            driver.execute_async_script("const done=arguments[0];chrome.storage.local.set({authorizedCookieHosts:['127.0.0.1']}).then(()=>done(true))")
        _fit_browser_window(driver)
        media_url = ('https://dash.akamaized.net/akamai/bbb_30fps/bbb_30fps.mpd' if args.public_dash
                     else origin + ('/chunks/download?token=browser-smoke' if args.media == 'file'
                                    else '/dash/manifest.mpd?token=browser-smoke' if args.media == 'dash'
                                    else '/hls/index.m3u8?token=browser-smoke'))
        if args.opaque_manifest:
            media_url = origin + '/opaque?token=browser-smoke'
        page_url = ('https://reference.dashif.org/dash.js/latest/samples/getting-started/manual-load-single-video.html'
                    if args.public_dash else origin + '/index.html?mode=' + args.media + '-mse')
        report['page'] = page_url
        report['manifest'] = media_url
        driver.set_page_load_timeout(45)
        if args.cross_iframe:
            page_url = origin + '/outer.html'
            report['page'] = page_url
            report['frame_page'] = frame_origin + '/index.html?mode=hls-mse'
        driver.get(page_url)
        if args.cross_iframe:
            driver.switch_to.frame(driver.find_element('tag name', 'iframe'))
        if args.public_dash:
            video = wait(lambda: driver.execute_script("const v=document.querySelector('video');return v?.readyState>=2?v:null"),
                         'Public player not ready', 30)
            driver.execute_script("arguments[0].muted=true;arguments[0].play().catch(()=>{})", video)

        def button():
            return driver.execute_script("return document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector('.video-download:not(.identifying)') || null")

        target = wait(button, 'Player not identified', 45)
        wait(lambda: driver.execute_script("return [...document.querySelectorAll('video')].some(v=>v.currentTime>0)"),
             'Player not actually playing', 20)
        if args.media == 'dash':
            # 元数据更新可替换元素；用当前位置发出真实鼠标点击，避免持有旧 WebElement。
            x, y = driver.execute_script("const r=document.querySelector('hls-downloader-media-panel').shadowRoot.querySelector('.video-more').getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2]")
            actions = ActionChains(driver)
            actions.w3c_actions.pointer_action.move_to_location(round(x), round(y))
            actions.click().perform()
            def quality_control():
                return driver.execute_script("""
                const root=document.querySelector('hls-downloader-media-panel').shadowRoot;
                return [...root.querySelectorAll('.item')].find(row=>row.querySelector('.resource-url')?.textContent.includes('manifest.mpd')
                    || row.querySelector('.resource-url')?.textContent.includes('bbb_30fps.mpd'))?.querySelector('.quality-select') || null;
                """)
            select = wait(quality_control, 'DASH quality choices missing', 20)
            choices = Select(select)
            options = [{'label': o.text, 'value': o.get_attribute('value')} for o in choices.options]
            option = next((o for o in options if o['label'].startswith(str(args.height)+'p')), None)
            assert option, f'Requested {args.height}p missing: {options}'
            choices.select_by_value(option['value'])
            # 网络识别补充元数据会替换列表节点，重读当前控件验证选择仍然保留。
            select = wait(quality_control, 'DASH quality control disappeared', 5)
            assert select.get_attribute('value') == option['value'], 'Quality selection was reset'
            record('real-page-quality-selected', {'height': args.height, 'label': option['label'],
                                                  'choices': [o['label'] for o in options]})
            target = driver.execute_script("return arguments[0].closest('.item').querySelector('.download')", select)
        if args.context_menu:
            assert args.browser == 'firefox', 'Native context menu fixture currently supports Firefox'
            link = driver.execute_script("""
                const link = document.createElement('a'); link.href = arguments[0];
                link.textContent = 'Download current media';
                link.style.cssText = 'position:fixed;left:10px;top:10px;z-index:2147483647;background:white;padding:12px';
                document.body.append(link); return link;
            """, media_url)
            ActionChains(driver).context_click(link).perform()
            driver.set_context('chrome')
            wait(lambda: driver.execute_script("""
                const menu = document.getElementById('contentAreaContextMenu');
                return menu?.state === 'open' && [...menu.querySelectorAll('menuitem')]
                    .some(item => item.getAttribute('label') === '使用 HLS Downloader 下载' && !item.hidden);
            """), 'Real browser download context menu missing', 10)
            driver.execute_script("""
                const menu = document.getElementById('contentAreaContextMenu');
                const item = [...menu.querySelectorAll('menuitem')]
                    .find(item => item.getAttribute('label') === '使用 HLS Downloader 下载' && !item.hidden);
                item.doCommand(); menu.hidePopup();
            """)
            driver.set_context('content')
            record('native-browser-context-menu-download-in-media-frame', media_url)
        else:
            ActionChains(driver).move_to_element(target).click().perform()
        hwnd = wait(presenter_window, 'Independent confirmation missing', 10)
        assert not user32.IsWindowVisible(workbench), 'Browser offer showed workbench'
        capture(hwnd, args.report.with_name(args.report.stem + '-confirm.png'))
        bounds = wintypes.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(bounds))
        record('confirmation-window-bounds', [bounds.left, bounds.top, bounds.right, bounds.bottom])
        record('browser-click-only-independent-confirmation', True)
        click_download_confirmation(input_mode=args.confirmation_input, evidence_path=args.report.with_name('confirmation.json'))
        report['confirmation_input']=args.confirmation_input
        from PIL import ImageGrab
        time.sleep(.4)
        ImageGrab.grab().save(args.report.with_name(args.report.stem+'-after-confirm-click.png'))
        wait(lambda: any(t['task_id'] not in initial_ids and t['url'] == media_url
                         for t in core({'type':'snapshot','request_id':1})['tasks']),
             'Desktop confirm click did not create task', 6)
        samples = []
        visual_samples = []
        visual_progress_seen = False
        hud_closed = False
        deadline = time.monotonic() + (240 if args.public_dash else 45)
        finished = None
        transfer_observation_ready.set()
        while time.monotonic() < deadline:
            tasks = core({'type': 'snapshot', 'request_id': 1})['tasks']
            task = next((t for t in tasks if t['task_id'] not in initial_ids and t['url'] == media_url), None)
            if task:
                created.add(task['task_id'])
                report['last_task'] = task
                assert task['status'] != 'failed', task.get('error_message')
                if args.media != 'file' and task['status'] == 'downloading' and task['downloaded_bytes'] > 0 and not hud_closed:
                    hud = presenter_window('下载进度')
                    assert hud, 'Transferring task has no visible HUD'
                    assert user32.GetWindowLongW(hud, -20) & 8, 'HUD is behind browser (not topmost)'
                    samples.append({'bytes': task['downloaded_bytes'], 'speed': task['speed_bytes_per_sec']})
                    if task['downloaded_bytes'] not in [item[0] for item in visual_samples]:
                        time.sleep(.5)
                        shot=args.report.with_name(args.report.stem + f'-progress-{len(visual_samples)}.png')
                        bounds=wintypes.RECT()
                        user32.GetWindowRect(hud,ctypes.byref(bounds))
                        ImageGrab.grab(bbox=(bounds.left,bounds.top,bounds.right,bounds.bottom)).save(shot)
                        visual_samples.append((task['downloaded_bytes'], shot))
                    if len(visual_samples) >= 2:
                        scale = user32.GetDpiForWindow(hud) / 96
                        first, second = [Image.open(path).convert('RGB') for _, path in (visual_samples[0], visual_samples[-1])]
                        region=(round(8*scale),round(110*scale),first.width-round(8*scale),round(140*scale))
                        # Core 快照与屏幕绘制异步，首帧可能已显示下一份快照；等待真实文字变化。
                        if not ImageChops.difference(first.crop(region),second.crop(region)).getbbox():
                            assert not user32.IsWindowVisible(workbench), 'Download reopened workbench'
                            time.sleep(.2)
                            continue
                        visual_progress_seen = True
                        record('actual-hud-transferred-text-changes', [str(path.name) for _,path in visual_samples])
                        rect = wintypes.RECT()
                        user32.GetWindowRect(hud, ctypes.byref(rect))
                        scale = user32.GetDpiForWindow(hud) / 96
                        user32.SetCursorPos(rect.right-round(26*scale), rect.top+round(27*scale))
                        user32.mouse_event(2, 0, 0, 0, 0)
                        user32.mouse_event(4, 0, 0, 0, 0)
                        wait(lambda: not presenter_window('下载进度'), 'Close did not hide HUD', 5)
                        time.sleep(.6)
                        assert not presenter_window('下载进度'), 'Progress event reopened dismissed HUD'
                        hud_closed = True
                        record('hud-close-keeps-download-running-and-does-not-reopen', True)
                if task['status'] == 'completed':
                    finished = task
                    break
            assert not user32.IsWindowVisible(workbench), 'Download reopened workbench'
            time.sleep(.2)
        assert finished, 'Media download did not finish'
        if args.media != 'file':
            assert len({s['bytes'] for s in samples}) >= 2, f'No changing in-flight progress: {samples}'
            assert visual_progress_seen, 'HUD bytes text never changed during transfer'
        else:
            assert hashlib.sha256(Path(finished['output_path']).read_bytes()).digest() == hashlib.sha256((site / 'stream.mp4').read_bytes()).digest(), 'Direct media bytes changed'
            record('extensionless-mime-media-cookie-download-byte-integrity', True)
        subprocess.run([str(args.ffmpeg), '-v', 'error', '-xerror', '-i', finished['output_path'], '-f', 'null', '-'], check=True, timeout=60)
        metadata = json.loads(subprocess.check_output([str(args.ffmpeg.with_name('ffprobe.exe')), '-v', 'error',
                              '-show_streams', '-show_format', '-of', 'json', finished['output_path']], timeout=30))
        if args.media == 'dash':
            video = next(s for s in metadata['streams'] if s['codec_type'] == 'video')
            assert video['height'] == args.height, f'Selected quality lost: {video}'
            assert any(s['codec_type'] == 'audio' for s in metadata['streams']), 'Separate audio missing'
            duration = float(metadata['format']['duration'])
            assert (590 < duration < 650) if args.public_dash else (3.9 < duration < 4.2), f'Incomplete media: {duration}s'
            if not args.public_dash:
                def frames(path):
                    output = subprocess.check_output([str(args.ffmpeg), '-v', 'error', '-i', str(path),
                                                       '-map', '0:v:0', '-f', 'framemd5', '-'], timeout=30).decode()
                    return [line.rsplit(',', 1)[-1].strip() for line in output.splitlines() if line and not line.startswith('#')]
                representation = '0' if args.height == 180 else '1'
                reference = stage / 'expected-video.mp4'
                with reference.open('wb') as stream:
                    for segment in [site / f'dash/init-{representation}.m4s', *sorted(site.glob(f'dash/chunk-{representation}-*.m4s'))]:
                        stream.write(segment.read_bytes())
                expected = frames(reference)
                assert len(expected) == 120 and frames(finished['output_path']) == expected, 'Video frames changed or lost'
                record('selected-video-frame-integrity', {'height': args.height, 'frames': len(expected)})
                audio_reference = stage / 'expected-audio.m4a'
                with audio_reference.open('wb') as stream:
                    for segment in [site / 'dash/init-2.m4s', *sorted(site.glob('dash/chunk-2-*.m4s'))]:
                        stream.write(segment.read_bytes())
                # AAC 的 edit list/预延迟标记会改变容器解码的首帧；逐包校验压缩音频数据。
                def audio_packets(path):
                    data = json.loads(subprocess.check_output([str(args.ffmpeg.with_name('ffprobe.exe')),
                            '-v', 'error', '-select_streams', 'a:0', '-show_packets', '-show_data_hash', 'sha256',
                            '-of', 'json', str(path)], timeout=30))
                    return [packet['data_hash'] for packet in data['packets']]
                expected_audio = audio_packets(audio_reference)
                assert expected_audio and audio_packets(finished['output_path']) == expected_audio, 'Audio packets changed or lost'
                record('separate-audio-packet-integrity', {'packets': len(expected_audio), 'hashes': expected_audio})
        output = Path(finished['output_path'])
        record('fully-decoded-direct-media-output' if args.media == 'file' else 'in-flight-progress-and-fully-decoded-output', {'samples': samples, 'metadata': metadata,
               'bytes': output.stat().st_size, 'sha256': hashlib.sha256(output.read_bytes()).hexdigest()})
        if args.authenticated:
            requests = [r for r in report['http_requests'] if r['core']]
            assert requests and not any(r.get('rejected') for r in requests), 'Core sent incorrect credentials'
            required = [('/chunks/', 'chunks')] if args.media == 'file' else [('/opaque?' if args.opaque_manifest else '/hls/' if args.media == 'hls' else '/dash/', 'manifest'), ('/init/', 'init'), ('/chunks/', 'chunks')]
            for prefix, cookie in required:
                assert any(r['path'].startswith(prefix) and r['cookie'] == f'hlsqa={cookie}' for r in requests), prefix
            record('per-path-httponly-cookie-and-real-frame-referer', requests)
        assert not user32.IsWindowVisible(workbench), 'Completion reopened workbench'

        if args.public_dash or args.media == 'file':
            report['passed'] = True
            return

        driver.switch_to.default_content()
        driver.get(origin + '/unresolved.html')
        wait(lambda: driver.execute_script("return document.querySelector('video').currentTime > 0"), 'Blob video not playing', 15)
        unresolved = wait(lambda: driver.execute_script("return document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector('.video-download') || null"), 'Unresolved overlay missing', 10)
        ActionChains(driver).move_to_element(unresolved).perform()
        wait(lambda: driver.execute_script("return document.querySelector('hls-downloader-media-panel').shadowRoot.querySelector('.video-download').textContent.includes('重新识别')"), 'Recognition never terminates', 18)
        driver.save_screenshot(str(args.report.with_name(args.report.stem + '-unresolved.png')))
        target = driver.execute_script("return document.querySelector('hls-downloader-media-panel').shadowRoot.querySelector('.video-download')")
        ActionChains(driver).move_to_element(target).click().perform()
        wait(lambda: driver.execute_script("return document.querySelector('hls-downloader-media-panel').shadowRoot.querySelector('.video-download').textContent.includes('正在识别')"), 'Rescan does not restart', 5)
        record('unresolved-playing-blob-times-out-while-hovering-and-can-rescan', True)
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
        if driver:
            try:
                driver.save_screenshot(str(args.report.with_name(args.report.stem + '-failure.png')))
                report['page_state'] = driver.execute_script("""
                    return {text:document.body.innerText.slice(0,4000),
                        overlay:document.querySelector('hls-downloader-media-panel')?.shadowRoot?.textContent.slice(-4000),
                        videos:[...document.querySelectorAll('video')].map(v=>({src:v.currentSrc,ready:v.readyState,time:v.currentTime,paused:v.paused,error:v.error?.message}))};
                """)
            except Exception as diagnostic_error:
                report['diagnostic_error'] = str(diagnostic_error)
        raise
    finally:
        if driver:
            driver.quit()
        if server:
            server.shutdown()
            server.server_close()
        if frame_server:
            frame_server.shutdown()
            frame_server.server_close()
        if temporary:
            temporary.cleanup()
        if origin:
            handoffs=[json.loads(x) for x in core({'type':'load_handoffs','request_id':1})['items']]
            for item in handoffs:
                if item.get('status')=='pending' and (item.get('offer',{}).get('url','').startswith(origin+'/')
                                                     or item.get('offer',{}).get('url') == media_url):
                    command({'kind':'reject_handoff','handoff_id':item['id'],'suppress_site_kind':False})
        # 创建之后尚未进入轮询就报错时，也必须回收本轮任务。
        created.update(t['task_id'] for t in core({'type':'snapshot','request_id':1})['tasks']
                       if t['task_id'] not in initial_ids and t['url'] == media_url)
        for task_id in created:
            command({'kind': 'task_action', 'task_id': task_id, 'action': 'delete_files'})
        if was_visible:
            command({'kind': 'open_main'})
        after = core({'type': 'snapshot', 'request_id': 1})['tasks']
        assert initial_ids <= {t['task_id'] for t in after}, 'Original task lost'
        report['user_tasks_preserved'] = sorted(initial_ids)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--browser', choices=['firefox', 'edge'], required=True)
    parser.add_argument('--extension', type=Path, required=True)
    parser.add_argument('--driver', type=Path, required=True)
    parser.add_argument('--ffmpeg', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--confirmation-input', choices=['system', 'window'], default='system')
    parser.add_argument('--media', choices=['hls', 'dash', 'file'], default='hls')
    parser.add_argument('--height', type=int, choices=[180, 360], default=180)
    parser.add_argument('--public-dash', action='store_true')
    parser.add_argument('--authenticated', action='store_true', help='Require different HttpOnly cookies for manifest, init and chunks')
    parser.add_argument('--cross-iframe', action='store_true')
    parser.add_argument('--redirect-init', action='store_true')
    parser.add_argument('--authenticated-headers', action='store_true')
    parser.add_argument('--opaque-manifest', action='store_true')
    parser.add_argument('--context-menu', action='store_true')
    args = parser.parse_args()
    if args.public_dash:
        args.media = 'dash'
    run(args)
