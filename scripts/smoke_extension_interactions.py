"""Actual Firefox public-page and extension button interactions, with UI metrics."""
from __future__ import annotations

import argparse
import contextlib
import ctypes
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading
import time
import traceback
from ctypes import wintypes
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit
import zipfile

from selenium import webdriver
from selenium.webdriver import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys
from selenium.webdriver.support.ui import Select
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service
from selenium.webdriver.edge.options import Options as EdgeOptions
from selenium.webdriver.edge.service import Service as EdgeService
from selenium.common.exceptions import StaleElementReferenceException

from smoke_extension_media import _find_firefox, _find_edge, _fit_browser_window
from smoke_v7_portable_app import Pipe, wait
from smoke_v7_browser_download_confirmation import click_download_confirmation


def test_replay_context(protected):
    # 只读取本次测试新建交接的凭据；不枚举或输出用户凭据。
    class Blob(ctypes.Structure):
        _fields_ = [('size', ctypes.c_uint32), ('data', ctypes.POINTER(ctypes.c_ubyte))]
    raw = bytes.fromhex(protected.removeprefix('dpapi:'))
    source = (ctypes.c_ubyte * len(raw)).from_buffer_copy(raw)
    output = Blob()
    crypt = ctypes.WinDLL('crypt32', use_last_error=True)
    crypt.CryptUnprotectData.argtypes = [ctypes.POINTER(Blob), ctypes.c_void_p, ctypes.c_void_p,
                                       ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(Blob)]
    if not crypt.CryptUnprotectData(ctypes.byref(Blob(len(raw), source)), None, None, None, None, 1, ctypes.byref(output)):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        return json.loads(ctypes.string_at(output.data, output.size))
    finally:
        kernel = ctypes.WinDLL('kernel32')
        kernel.LocalFree.argtypes = [ctypes.c_void_p]
        kernel.LocalFree(output.data)


def run(args):
    args.report.parent.mkdir(parents=True, exist_ok=True)
    evidence = args.report.parent
    result = {"passed": False, "browser": args.browser, "checks": [], "buttons": []}
    result['engine_sha256']=hashlib.sha256((args.runtime/'app/resources/HLSDownloaderEngine.exe').read_bytes()).hexdigest()
    result['extension_sha256']={f.relative_to(args.extension).as_posix():hashlib.sha256(f.read_bytes()).hexdigest()
        for f in sorted(args.extension.rglob('*')) if f.is_file()}
    driver = None
    video_server = None
    video_server_started = False
    original_tasks = set()
    created = set()
    original_settings = None
    test_handoff_id = ''
    before_handoffs = set()
    fixture_certificate_thumbprint = None
    fixture_certificate_cleanup_needed = False

    def run_certutil(arguments):
        process = subprocess.Popen(
            ['certutil.exe', *arguments],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0),
        )
        try:
            stdout, stderr = process.communicate(timeout=30)
            timed_out = False
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                stdout, stderr = process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                stdout, stderr = process.communicate()
            timed_out = True
        return {
            'exit_code': process.returncode,
            'stdout': stdout,
            'stderr': stderr,
            'timed_out': timed_out,
        }

    def core(message):
        p = Pipe()
        try:
            return p.request(message)
        finally:
            p.close()

    def record(name, actual=True):
        result["checks"].append({"name": name, "actual": actual})
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf8")
        print(json.dumps({"check": name}, ensure_ascii=True), flush=True)

    def message(value):
        return driver.execute_async_script(
            "const [m,done]=arguments;(globalThis.browser||chrome).runtime.sendMessage(m).then(done,e=>done({ok:false,error:String(e)}))", value)

    def zoom_page(value):
        if args.browser == 'firefox':
            driver.set_context('chrome')
            driver.execute_script('ZoomManager.zoom=arguments[0]', value)
            driver.set_context('content')
            wait(find_video, 'Player iframe missing after page zoom', 15)
        else:
            player = driver.current_window_handle
            driver.switch_to.new_window('tab')
            try:
                driver.get('chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/popup.html')
                response = driver.execute_async_script('''
                    const [page,zoom,done]=arguments;
                    chrome.tabs.query({}).then(tabs=>{
                        const tab=tabs.find(tab=>tab.url===page);
                        if(!tab)throw new Error('Player tab not found');
                        return chrome.tabs.setZoom(tab.id,zoom);
                    }).then(()=>done({ok:true}),error=>done({error:String(error)}));
                ''', PAGE, value)
                assert response.get('ok'), response
            finally:
                driver.close()
                driver.switch_to.window(player)
                wait(find_video, 'Player iframe missing after page zoom', 15)

    def popup_button(text):
        return wait(lambda: next((b for b in driver.find_elements(By.TAG_NAME, "button")
                                 if b.is_displayed() and (text in b.text or text in (b.get_attribute('aria-label') or ''))), None), "Missing popup button: " + text, 12)

    def styles(element):
        return driver.execute_script("""
            const e=arguments[0],r=e.getBoundingClientRect(),s=getComputedStyle(e);
            return {text:e.textContent.trim(),title:e.title,label:e.getAttribute('aria-label'),
                    width:r.width,height:r.height,x:r.x,y:r.y,disabled:e.disabled,
                    hovered:e.matches(':hover'),active:e.matches(':active'),group:e.closest('.video-action-group')?.className,
                    background:s.backgroundColor,color:s.color,transform:s.transform,
                    border:s.borderColor,shadow:s.boxShadow,outline:s.outlineColor};
        """, element)

    def drag_viewports():
        return driver.execute_script("""
            const frame=window.frameElement;
            if(!frame)return {child:{width:innerWidth,height:innerHeight},top:null};
            const rect=frame.getBoundingClientRect();
            const parent=frame.ownerDocument.defaultView;
            const left=rect.left+frame.clientLeft,top=rect.top+frame.clientTop;
            const width=frame.clientWidth,height=frame.clientHeight;
            const scaleX=width/innerWidth,scaleY=height/innerHeight;
            const visible={left:Math.max(0,left),top:Math.max(0,top),
                right:Math.min(parent.innerWidth,left+width),bottom:Math.min(parent.innerHeight,top+height)};
            return {
                child:{width:innerWidth,height:innerHeight,devicePixelRatio},
                top:{width:parent.innerWidth,height:parent.innerHeight,devicePixelRatio:parent.devicePixelRatio},
                frame:{left,top,width,height,scaleX,scaleY,
                    visibleChild:{left:Math.max(0,(visible.left-left)/scaleX),
                        top:Math.max(0,(visible.top-top)/scaleY),
                        right:Math.min(innerWidth,(visible.right-left)/scaleX),
                        bottom:Math.min(innerHeight,(visible.bottom-top)/scaleY)}}
            };
        """)

    def zoom_target_diagnostics(selector):
        return driver.execute_script("""
            const host=document.querySelector('hls-downloader-media-panel');
            const target=host?.shadowRoot?.querySelector(arguments[0]);
            const video=document.querySelector('video');
            const rect=e=>{const r=e.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}};
            const describe=e=>e?{tag:e.tagName,id:e.id||'',className:typeof e.className==='string'?e.className:''}:null;
            if(!target)return {targetMissing:true,selector:arguments[0],video:video?rect(video):null,
                childViewport:{width:innerWidth,height:innerHeight},parentFrame:{readable:false,reason:'target missing'}};
            const tr=rect(target), vr=video?rect(video):null;
            const x=tr.x+tr.width/2,y=tr.y+tr.height/2;
            const root=target.getRootNode();
            const childHit=root.elementFromPoint?root.elementFromPoint(x,y):document.elementFromPoint(x,y);
            const out={childViewport:{width:innerWidth,height:innerHeight},video:vr,target:tr,
                childCenter:{x,y,insideViewport:x>=0&&y>=0&&x<innerWidth&&y<innerHeight},
                childHit:describe(childHit),documentHit:describe(document.elementFromPoint(x,y)),
                childHitIsTarget:!!childHit&&target.contains(childHit),
                targetVisible:tr.width>0&&tr.height>0&&getComputedStyle(target).visibility!=='hidden'&&getComputedStyle(target).display!=='none',
                parentFrame:{readable:false,reason:'window.frameElement unavailable'}};
            try {
                const frame=window.frameElement;
                if(frame){
                    const doc=frame.ownerDocument,win=doc.defaultView,r=frame.getBoundingClientRect();
                    const left=r.left+frame.clientLeft,top=r.top+frame.clientTop;
                    const width=frame.clientWidth,height=frame.clientHeight;
                    const scaleX=width/innerWidth,scaleY=height/innerHeight;
                    const px=left+x*scaleX,py=top+y*scaleY;
                    const visible={left:Math.max(0,left),top:Math.max(0,top),
                        right:Math.min(win.innerWidth,left+width),bottom:Math.min(win.innerHeight,top+height)};
                    visible.width=Math.max(0,visible.right-visible.left);
                    visible.height=Math.max(0,visible.bottom-visible.top);
                    const parentHit=doc.elementFromPoint(px,py);
                    out.parentFrame={readable:true,rect:{x:r.x,y:r.y,width:r.width,height:r.height},
                        contentRect:{x:left,y:top,width,height},parentViewport:{width:win.innerWidth,height:win.innerHeight},
                        visibleIntersection:visible,mappedTargetCenter:{x:px,y:py},
                        centerInVisibleIntersection:px>=visible.left&&px<visible.right&&py>=visible.top&&py<visible.bottom,
                        parentHit:describe(parentHit),parentHitIsFrame:parentHit===frame};
                }
            } catch(error) {
                out.parentFrame={readable:false,reason:String(error)};
            }
            return out;
        """, selector)

    def prepare_zoom_target(selector, context):
        def scroll_video():
            driver.execute_script("""
                const video=document.querySelector('video');
                if(!video)throw new Error('Local fixture video is unavailable in the active frame');
                video.scrollIntoView({block:'center',inline:'center'});
            """)

        def wait_for_stable_layout():
            previous = None
            stable_samples = 0

            def layout_stable():
                nonlocal previous, stable_samples
                state = driver.execute_script("""
                    const host=document.querySelector('hls-downloader-media-panel');
                    const target=host?.shadowRoot?.querySelector(arguments[0]);
                    const video=document.querySelector('video');
                    const rect=e=>{const r=e.getBoundingClientRect();return [r.x,r.y,r.width,r.height].map(v=>Math.round(v*100)/100)};
                    if(!target||!video)return null;
                    return {viewport:[innerWidth,innerHeight],video:rect(video),target:rect(target)};
                """, selector)
                if state is None:
                    previous = None
                    stable_samples = 0
                    return False
                if state == previous:
                    stable_samples += 1
                else:
                    previous = state
                    stable_samples = 0
                return stable_samples >= 3

            wait(layout_stable, 'Zoomed fixture video and controls did not settle', 10)

        def capture(stage):
            diagnostic = zoom_target_diagnostics(selector)
            result.setdefault('zoom_diagnostics', []).append({'context':context,'stage':stage,**diagnostic})
            args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf8')
            return diagnostic

        wait(find_video, 'Player iframe missing before zoom interaction', 15)
        scroll_video()
        wait_for_stable_layout()
        diagnostic = capture('after-video-scroll')
        if not diagnostic['parentFrame']['readable']:
            raise RuntimeError(f'{context}: parent iframe visibility is unreadable: {diagnostic}')
        if not diagnostic['parentFrame']['centerInVisibleIntersection'] or not diagnostic['parentFrame']['parentHitIsFrame']:
            driver.execute_script("""
                const frame=window.frameElement;
                if(!frame)throw new Error('Fixture output iframe is unavailable for parent-page scrolling');
                const doc=frame.ownerDocument,win=doc.defaultView;
                const host=document.querySelector('hls-downloader-media-panel');
                const target=host?.shadowRoot?.querySelector(arguments[0]);
                if(!target)throw new Error('Zoom interaction target disappeared before parent-page scrolling');
                const r=frame.getBoundingClientRect();
                const left=r.left+frame.clientLeft,top=r.top+frame.clientTop;
                const scaleX=frame.clientWidth/innerWidth,scaleY=frame.clientHeight/innerHeight;
                const tr=target.getBoundingClientRect();
                const x=left+(tr.left+tr.width/2)*scaleX;
                const y=top+(tr.top+tr.height/2)*scaleY;
                const dx=x<0||x>=win.innerWidth?x-win.innerWidth/2:0;
                const dy=y<0||y>=win.innerHeight?y-win.innerHeight/2:0;
                win.scrollBy(dx,dy);
            """, selector)
            wait(find_video, 'Player iframe missing after parent-page scroll', 15)
            wait_for_stable_layout()
            diagnostic = capture('after-parent-frame-scroll')

        if diagnostic.get('targetMissing'):
            raise RuntimeError(f'{context}: target disappeared while the fixture layout rebuilt: {diagnostic}')
        if not diagnostic['childCenter']['insideViewport'] or not diagnostic['childHitIsTarget']:
            raise RuntimeError(f'{context}: target center is outside the visible child target: {diagnostic}')
        if not diagnostic['parentFrame']['centerInVisibleIntersection'] or not diagnostic['parentFrame']['parentHitIsFrame']:
            raise RuntimeError(f'{context}: target center is outside the parent iframe visible intersection: {diagnostic}')
        if not diagnostic['targetVisible']:
            raise RuntimeError(f'{context}: target is not displayed after scrolling: {diagnostic}')
        return diagnostic

    def zoom_action(selector, context, click=False):
        for attempt in range(3):
            try:
                diagnostic = prepare_zoom_target(selector, context)
                point = diagnostic['parentFrame']['mappedTargetCenter']
                viewport = diagnostic['parentFrame']['parentViewport']
                x, y = round(point['x']), round(point['y'])
                if not (0 <= x < viewport['width'] and 0 <= y < viewport['height']):
                    raise RuntimeError(f'{context}: mapped mouse point is outside the top viewport: {diagnostic}')
                driver.switch_to.default_content()
                action = ActionChains(driver)
                action.w3c_actions.pointer_action.move_to_location(x, y)
                if click:
                    action.pause(.1)
                    action.w3c_actions.pointer_action.click()
                action.perform()
                wait(find_video, 'Player iframe missing after zoom interaction', 15)
                return
            except StaleElementReferenceException:
                if attempt == 2:
                    raise

    def measure(element, context):
        if not element.is_displayed():
            return
        driver.execute_script("arguments[0].scrollIntoView({block:'center',inline:'nearest'})", element)
        time.sleep(.15)
        ActionChains(driver).move_by_offset(0, 0).perform()
        driver.execute_script("document.activeElement?.blur()")
        neutral = ActionChains(driver)
        neutral.w3c_actions.pointer_action.move_to_location(2, 2)
        neutral.perform()
        driver.get_screenshot_as_png()
        diagnostic = driver.execute_script("return {hover:arguments[0].matches(':hover'),animations:arguments[0].getAnimations().map(a=>({state:a.playState,time:a.currentTime,type:a.constructor.name})),title:arguments[0].title}",element)
        if diagnostic["hover"]:
            print(json.dumps({"neutral_pointer":diagnostic}),flush=True)
        time.sleep(.25)
        rest = styles(element)
        ActionChains(driver).move_to_element(element).perform()
        driver.get_screenshot_as_png()
        time.sleep(.25)
        # 初次资源补齐会改变按钮组宽度；按当前布局重新移动，避免测到旧坐标。
        for _ in range(3):
            if driver.execute_script("return arguments[0].matches(':hover')", element):
                break
            ActionChains(driver).move_to_element(element).perform()
            time.sleep(.2)
        assert driver.execute_script("return arguments[0].matches(':hover')", element), ("Pointer missed moving button", context)
        hover = styles(element)
        ActionChains(driver).click_and_hold(element).perform()
        time.sleep(.18)
        press = styles(element)
        neutral = ActionChains(driver)
        neutral.w3c_actions.pointer_action.move_to_location(2, 2)
        neutral.release().perform()
        wait(lambda:not driver.execute_script("return arguments[0].closest('.video-action-group')?.classList.contains('dragging')",element),
            'Pointer release left the video controls dragging',5)
        fields = ("background", "color", "transform", "border", "shadow", "outline")
        item = {"context": context, "rest": rest, "hover": hover, "press": press,
                "hover_changes": [f for f in fields if rest[f] != hover[f]],
                "press_changes": [f for f in fields if hover[f] != press[f]]}
        result["buttons"].append(item)
        if not rest["disabled"]:
            assert rest["width"] >= 24 and rest["height"] >= 24, (context, rest)
            assert item["hover_changes"], ("missing hover feedback", context, rest)
            assert item["press_changes"], ("missing press feedback", context, rest)

    def measure_stable(locator, context):
        # 播放器初始化和资源扫描完成时允许一次 DOM 换代，重新定位后测同一按钮。
        for attempt in range(3):
            try:
                return measure(locator(), context)
            except StaleElementReferenceException:
                neutral = ActionChains(driver)
                neutral.w3c_actions.pointer_action.move_to_location(2, 2)
                neutral.release().perform()
                if attempt == 2:
                    raise

    with tempfile.TemporaryDirectory(prefix="all-buttons-firefox-", dir=args.runtime.resolve().parents[1]/"test-tmp") as temporary:
        stage = Path(temporary)
        try:
            fixture_root = stage / 'video-fixture'
            fixture_root.mkdir()
            video_path = fixture_root / 'stream.mp4'
            subprocess.run([
                str(args.runtime.resolve() / 'app/resources/ffmpeg.exe'), '-hide_banner', '-loglevel', 'error', '-y',
                '-f', 'lavfi', '-i', 'testsrc=size=640x360:rate=30',
                '-f', 'lavfi', '-i', 'sine=frequency=880:sample_rate=48000', '-t', '4',
                '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-g', '30', '-c:a', 'aac',
                '-movflags', '+faststart', str(video_path),
            ], check=True, timeout=60, capture_output=True)
            expected_sha256 = hashlib.sha256(video_path.read_bytes()).hexdigest()
            (fixture_root / 'index.html').write_text(
                '<!doctype html><meta charset="utf-8"><title>Local media fixture</title>'
                '<iframe src="/player.html" width="760" height="520" title="Video player"></iframe>',
                encoding='utf-8',
            )
            (fixture_root / 'player.html').write_text(
                '<!doctype html><meta charset="utf-8"><video controls autoplay muted loop '
                'width="640" height="360" src="/stream.mp4"></video>',
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
            fixture_scheme = 'http'
            if args.partitioned_cookies:
                if os.environ.get('HLS_VM_ACCEPTANCE') != '1':
                    raise RuntimeError(
                        'Partitioned-cookie fixture requires the isolated HLS VM acceptance guest'
                    )
                import datetime
                from cryptography import x509
                from cryptography.hazmat.primitives import hashes, serialization
                from cryptography.hazmat.primitives.asymmetric import rsa
                from cryptography.x509.oid import NameOID

                now = datetime.datetime.utcnow()
                key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
                subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'localhost')])
                certificate = (
                    x509.CertificateBuilder()
                    .subject_name(subject)
                    .issuer_name(subject)
                    .public_key(key.public_key())
                    .serial_number(x509.random_serial_number())
                    .not_valid_before(now - datetime.timedelta(minutes=5))
                    .not_valid_after(now + datetime.timedelta(days=1))
                    .add_extension(x509.SubjectAlternativeName([x509.DNSName('localhost')]), critical=False)
                    .sign(key, hashes.SHA256())
                )
                cert_path = stage / 'localhost.pem'
                key_path = stage / 'localhost-key.pem'
                cert_path.write_bytes(certificate.public_bytes(serialization.Encoding.PEM))
                key_path.write_bytes(key.private_bytes(
                    serialization.Encoding.PEM,
                    serialization.PrivateFormat.PKCS8,
                    serialization.NoEncryption(),
                ))
                fixture_certificate_thumbprint = certificate.fingerprint(hashes.SHA1()).hex().upper()
                fixture_certificate_cleanup_needed = True
                try:
                    store_result = run_certutil(
                        ['-f', '-addstore', 'Root', str(cert_path)]
                    )
                    result['fixture_certificate_store'] = {
                        'thumbprint': fixture_certificate_thumbprint,
                        'store_scope': 'LocalMachine\\Root',
                        'add_exit_code': store_result['exit_code'],
                        'add_stdout': store_result['stdout'],
                        'add_stderr': store_result['stderr'],
                        'add_timed_out': store_result['timed_out'],
                    }
                except OSError as error:
                    result['fixture_certificate_store'] = {
                        'thumbprint': fixture_certificate_thumbprint,
                        'add_error': str(error),
                    }
                    raise RuntimeError('Could not start certutil for the temporary fixture certificate') from error
                if store_result['timed_out'] or store_result['exit_code'] != 0:
                    raise RuntimeError(
                        f'Could not trust the temporary fixture certificate '
                        f'(certutil exit {store_result["exit_code"]}, '
                        f'timed out={store_result["timed_out"]})'
                    )
                tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
                tls.load_cert_chain(str(cert_path), str(key_path))
                video_server.socket = tls.wrap_socket(video_server.socket, server_side=True)
                fixture_scheme = 'https'
            threading.Thread(target=video_server.serve_forever, daemon=True).start()
            video_server_started = True
            PAGE = f'{fixture_scheme}://localhost:{video_server.server_port}/index.html'
            MEDIA = f'{fixture_scheme}://localhost:{video_server.server_port}/stream.mp4'
            media_origin = urlsplit(MEDIA).scheme + '://' + urlsplit(MEDIA).netloc
            result['page'] = PAGE
            original_settings = core({"type": "load_settings", "request_id": 1})
            original_tasks = {t["task_id"] for t in core({"type":"snapshot","request_id":2})["tasks"]}
            addon = stage / "extension.xpi"
            with zipfile.ZipFile(addon, "w", zipfile.ZIP_DEFLATED) as archive:
                for f in sorted(args.extension.resolve().rglob("*")):
                    if f.is_file(): archive.write(f, f.relative_to(args.extension.resolve()).as_posix())
            popup_url = 'chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/popup.html'
            if args.browser == 'firefox':
                options = Options()
                options.binary_location = str(_find_firefox())
                options.accept_insecure_certs = args.partitioned_cookies
                options.set_preference('browser.shell.checkDefaultBrowser', False)
                options.set_preference('browser.aboutwelcome.enabled', False)
                options.set_preference(
                    'browser.newtabpage.activity-stream.asrouter.providers.onboarding',
                    json.dumps({'id': 'onboarding', 'type': 'local',
                                'localProvider': 'OnboardingMessageProvider', 'enabled': False, 'exclude': []}))
                options.set_preference(
                    'browser.newtabpage.activity-stream.asrouter.providers.messaging-experiments',
                    json.dumps({'id': 'messaging-experiments', 'enabled': False,
                                'type': 'remote-experiments', 'updateCycleInMs': 3600000}))
                if not args.headed:
                    options.add_argument('-headless')
                options.set_preference('media.autoplay.default', 0)
                service = Service(executable_path=str(args.driver),
                                  service_args=['--profile-root', str(stage), '--allow-system-access'],
                                  log_output=str(evidence / 'geckodriver.log'))
                driver = webdriver.Firefox(options=options, service=service)
                eid = driver.install_addon(str(addon), temporary=True)
                driver.set_context('chrome')
                uuid = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))[eid]
                driver.set_context('content')
                popup_url = f'moz-extension://{uuid}/popup.html'
            else:
                options = EdgeOptions()
                options.binary_location = str(_find_edge())
                options.accept_insecure_certs = args.partitioned_cookies
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
            driver.set_script_timeout(20)
            driver.get(PAGE)
            player = driver.current_window_handle
            def find_video():
                driver.switch_to.default_content()
                if driver.find_elements(By.TAG_NAME, "video"):
                    return True
                for frame in driver.find_elements(By.TAG_NAME, "iframe"):
                    try:
                        driver.switch_to.frame(frame)
                        if driver.find_elements(By.TAG_NAME, "video"):
                            return True
                        driver.switch_to.default_content()
                    except StaleElementReferenceException:
                        driver.switch_to.default_content()
                        return False
                return False
            wait(find_video, "Public page contains no video player", 20)
            driver.execute_script("document.querySelector('video').muted=true;document.querySelector('video').play()")
            wait(lambda: driver.execute_script("return Boolean(document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector('.video-download:not(.identifying)'))"), "Public video not identified", 40)
            driver.execute_script("""
                window.__hlsUiErrors=[];window.__hlsPointerEvents=[];
                window.addEventListener('error',e=>window.__hlsUiErrors.push(e.message));
                for(const type of ['pointerdown','pointerup','pointercancel','lostpointercapture'])
                    window.addEventListener(type,e=>window.__hlsPointerEvents.push({type,id:e.pointerId,buttons:e.buttons,target:e.composedPath()[0]?.className}),true);
            """)
            driver.switch_to.default_content()
            driver.execute_script("""
                window.__hlsShortcutCloseEvents=[];
                for(const type of ['pointerdown','pointerup','click'])
                    window.addEventListener(type,e=>window.__hlsShortcutCloseEvents.push({
                        type,id:e.pointerId??null,buttons:e.buttons??0,detail:e.detail??0,
                        x:e.clientX,y:e.clientY,
                        target:(()=>{const t=e.composedPath()[0];return t?{tag:t.tagName,className:typeof t.className==='string'?t.className:'',title:t.title||''}:null})()
                    }),true);
            """)
            if not find_video():
                raise RuntimeError('Player iframe was not restored after installing top-level pointer diagnostics')

            def overlay(selector):
                return driver.execute_script("return document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector(arguments[0])||null", selector)

            if args.shortcut:
                close = overlay('.close')
                if close and close.is_displayed(): close.click()
                driver.switch_to.default_content()
                top_panel_before = driver.execute_script("""
                    const visible=e=>{const s=getComputedStyle(e);return s.display!=='none'&&s.visibility!=='hidden'&&e.getClientRects().length>0};
                    return [...document.querySelectorAll('hls-downloader-media-panel')]
                        .filter(host=>visible(host.shadowRoot?.querySelector('.wrap'))).length;
                """)
                assert top_panel_before == 0, {'top_level_visible_panels_before': top_panel_before}
                if not find_video():
                    raise RuntimeError('Player iframe was not available before shortcut scope check')
                iframe_panel_before = bool(driver.execute_script("""
                    const panel=document.querySelector('hls-downloader-media-panel')?.shadowRoot?.querySelector('.wrap');
                    if(!panel)return false;
                    const style=getComputedStyle(panel);
                    return style.display!=='none'&&style.visibility!=='hidden'&&panel.getClientRects().length>0;
                """))
                assert not iframe_panel_before, 'Player iframe media panel was already open before shortcut'
                if args.browser == 'edge':
                    if driver.current_url != PAGE:
                        raise RuntimeError('Edge shortcut attempt requires the local fixture page')
                    user32 = ctypes.WinDLL('user32', use_last_error=True)
                    user32.EnumWindows.argtypes = [ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM), wintypes.LPARAM]
                    user32.EnumWindows.restype = wintypes.BOOL
                    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
                    user32.GetForegroundWindow.restype = wintypes.HWND
                    user32.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]
                    user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
                    user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
                    user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
                    windows = []
                    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
                    def find_edge_window(hwnd, _):
                        if not user32.IsWindowVisible(hwnd):
                            return True
                        class_name = ctypes.create_unicode_buffer(256)
                        user32.GetClassNameW(hwnd, class_name, len(class_name))
                        if class_name.value != 'Chrome_WidgetWin_1':
                            return True
                        length = user32.GetWindowTextLengthW(hwnd)
                        title = ctypes.create_unicode_buffer(length + 1)
                        user32.GetWindowTextW(hwnd, title, length + 1)
                        if driver.title and driver.title in title.value:
                            owner = wintypes.DWORD()
                            user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
                            windows.append({'hwnd': hwnd, 'pid': owner.value, 'title': title.value})
                        return True
                    callback = callback_type(find_edge_window)
                    user32.EnumWindows(callback, 0)
                    assert windows, ('Edge window for current page not found', driver.title)
                    browser = windows[0]
                    user32.SetWindowPos(browser['hwnd'], wintypes.HWND(-1), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0040)
                    user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
                    user32.WindowFromPoint.argtypes = [wintypes.POINT]
                    user32.WindowFromPoint.restype = wintypes.HWND
                    user32.GetAncestor.argtypes = [wintypes.HWND, wintypes.UINT]
                    user32.GetAncestor.restype = wintypes.HWND
                    user32.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
                    user32.SetCursorPos.restype = wintypes.BOOL
                    user32.mouse_event.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.DWORD,
                                                   wintypes.DWORD, ctypes.c_size_t]
                    browser_rect = wintypes.RECT()
                    assert user32.GetWindowRect(browser['hwnd'], ctypes.byref(browser_rect)), 'Edge window bounds unavailable'
                    titlebar_point = wintypes.POINT((browser_rect.left + browser_rect.right) // 2,
                                                    browser_rect.top + 8)
                    assert user32.SetCursorPos(titlebar_point.x, titlebar_point.y), 'Could not move to Edge title bar'
                    titlebar_hit = user32.WindowFromPoint(titlebar_point)
                    titlebar_root = user32.GetAncestor(titlebar_hit, 2) if titlebar_hit else None
                    titlebar_pid = wintypes.DWORD()
                    if titlebar_root:
                        user32.GetWindowThreadProcessId(titlebar_root, ctypes.byref(titlebar_pid))
                    assert titlebar_root == browser['hwnd'] and titlebar_pid.value == browser['pid'], (
                        'Edge title-bar point missed the selected browser window',
                        {'browser_hwnd': int(browser['hwnd']), 'browser_pid': browser['pid'],
                         'hit_hwnd': int(titlebar_hit) if titlebar_hit else None,
                         'root_hwnd': int(titlebar_root) if titlebar_root else None,
                         'root_pid': titlebar_pid.value, 'point': [titlebar_point.x, titlebar_point.y]})
                    user32.mouse_event(0x0002, 0, 0, 0, 0)
                    user32.mouse_event(0x0004, 0, 0, 0, 0)
                    wait(lambda: user32.GetForegroundWindow() == browser['hwnd'], 'Edge page did not become foreground', 5)
                    driver.switch_to.default_content()
                    active_element = driver.execute_script("document.body.focus();return document.activeElement?.tagName")
                    result['shortcut_attempt'] = {
                        'method': 'verified title-bar click plus Win32 keybd_event', 'fixture': PAGE, 'keys': ['CTRL', 'SHIFT', 'Y'],
                        'window': {'hwnd': int(browser['hwnd']), 'pid': browser['pid'], 'title': browser['title']},
                        'titlebar_point': [titlebar_point.x, titlebar_point.y],
                        'foreground_hwnd_before': int(user32.GetForegroundWindow()),
                        'active_element': active_element,
                    }
                    pressed = []
                    try:
                        for key in (0x11, 0x10, 0x59):
                            user32.keybd_event(key, 0, 0, 0)
                            pressed.append(key)
                            time.sleep(.04)
                    finally:
                        for key in reversed(pressed):
                            user32.keybd_event(key, 0, 0x0002, 0)
                            time.sleep(.02)
                    result['shortcut_attempt']['foreground_hwnd_after'] = int(user32.GetForegroundWindow())
                else:
                    try:
                        driver.set_context('chrome')
                        ActionChains(driver) \
                            .key_down(Keys.CONTROL) \
                            .key_down(Keys.SHIFT) \
                            .send_keys('y') \
                            .key_up(Keys.SHIFT) \
                            .key_up(Keys.CONTROL) \
                            .perform()
                    finally:
                        driver.set_context('content')
                driver.switch_to.default_content()
                wait(lambda: driver.execute_script("""
                    const visible=e=>{const s=getComputedStyle(e);return s.display!=='none'&&s.visibility!=='hidden'&&e.getClientRects().length>0};
                    return [...document.querySelectorAll('hls-downloader-media-panel')]
                        .some(host=>visible(host.shadowRoot?.querySelector('.wrap')));
                """), 'Actual Ctrl+Shift+Y did not open exactly one top-level media panel', 10)
                top_panel_count = driver.execute_script("""
                    const visible=e=>{const s=getComputedStyle(e);return s.display!=='none'&&s.visibility!=='hidden'&&e.getClientRects().length>0};
                    return [...document.querySelectorAll('hls-downloader-media-panel')]
                        .filter(host=>visible(host.shadowRoot?.querySelector('.wrap'))).length;
                """)
                assert top_panel_count == 1, {'top_level_visible_panels': top_panel_count}
                if args.browser == 'edge':
                    result['shortcut_attempt'].update({'panel_visible_after': True, 'opened': True})
                if not find_video():
                    raise RuntimeError('Player iframe was not available to verify shortcut scope')
                iframe_panel_open = bool(driver.execute_script("""
                    const host=document.querySelector('hls-downloader-media-panel');
                    const panel=host?.shadowRoot?.querySelector('.wrap');
                    if(!panel)return false;
                    const style=getComputedStyle(panel);
                    return style.display!=='none'&&style.visibility!=='hidden'&&panel.getClientRects().length>0;
                """))
                assert not iframe_panel_open, 'Ctrl+Shift+Y also opened a media panel inside the player iframe'
                driver.switch_to.default_content()
                media_name = Path(urlsplit(MEDIA).path).name
                aggregated = wait(lambda: driver.execute_script("""
                    const name=arguments[0];
                    const visible=e=>{if(!e)return false;const s=getComputedStyle(e);return s.display!=='none'&&s.visibility!=='hidden'&&e.getClientRects().length>0};
                    const host=[...document.querySelectorAll('hls-downloader-media-panel')]
                        .find(item=>visible(item.shadowRoot?.querySelector('.wrap')));
                    if(!host)return null;
                    const urls=[...host.shadowRoot.querySelectorAll('.resource-url')]
                        .map(item=>item.textContent||'');
                    return urls.some(url=>url.includes(name)) ? {resource_urls:urls,media_name:name} : null;
                """, media_name), 'Top-level panel did not aggregate the iframe MEDIA resource', 20)

                def close_state():
                    return driver.execute_script("""
                        const visible=e=>{if(!e)return false;const s=getComputedStyle(e);return s.display!=='none'&&s.visibility!=='hidden'&&e.getClientRects().length>0};
                        const describe=e=>e?{tag:e.tagName,className:typeof e.className==='string'?e.className:'',title:e.title||''}:null;
                        const hosts=[...document.querySelectorAll('hls-downloader-media-panel')];
                        const host=hosts.find(item=>visible(item.shadowRoot?.querySelector('.wrap'))) || hosts[0];
                        const close=host?.shadowRoot?.querySelector('.close');
                        if(!host||!close)return {visible_panels:hosts.filter(item=>visible(item.shadowRoot?.querySelector('.wrap'))).length,
                            close:null,viewport:{width:innerWidth,height:innerHeight}};
                        const r=close.getBoundingClientRect(),x=r.x+r.width/2,y=r.y+r.height/2;
                        const shadowHit=host.shadowRoot.elementFromPoint(x,y);
                        const pageHit=document.elementFromPoint(x,y);
                        return {visible_panels:hosts.filter(item=>visible(item.shadowRoot?.querySelector('.wrap'))).length,
                            close:{x:r.x,y:r.y,width:r.width,height:r.height,center:{x,y},visible:visible(close),
                                shadow_hit:describe(shadowHit),shadow_hit_is_close:shadowHit===close,
                                shadow_hit_inside_close:!!shadowHit&&close.contains(shadowHit),page_hit:describe(pageHit)},
                            viewport:{width:innerWidth,height:innerHeight}};
                    """)

                previous_close = None
                stable_count = 0
                def stable_close_state():
                    nonlocal previous_close, stable_count
                    state = close_state()
                    close = state.get('close')
                    key = None if not close else (close['x'], close['y'], close['width'], close['height'])
                    if key is None or key != previous_close:
                        previous_close, stable_count = key, 0
                        return False
                    stable_count += 1
                    return state if stable_count >= 3 else False

                stable_close = wait(stable_close_state, 'Top-level close button geometry did not stabilize after resource aggregation', 15)
                assert stable_close['visible_panels'] == 1 and stable_close['close']['visible'], stable_close
                assert stable_close['close']['width'] > 0 and stable_close['close']['height'] > 0, stable_close
                assert 0 <= stable_close['close']['center']['x'] < stable_close['viewport']['width'] and 0 <= stable_close['close']['center']['y'] < stable_close['viewport']['height'], stable_close
                assert stable_close['close']['shadow_hit_inside_close'], stable_close

                click_attempts = []
                for _ in range(3):
                    before_move = close_state()
                    assert before_move.get('close') and before_move['close']['shadow_hit_inside_close'], before_move
                    x = round(before_move['close']['center']['x'])
                    y = round(before_move['close']['center']['y'])
                    mover = ActionChains(driver)
                    mover.w3c_actions.pointer_action.move_to_location(x, y)
                    mover.perform()
                    before_click = close_state()
                    click_attempts.append({'before_move': before_move, 'before_click': before_click})
                    if (before_click.get('close') and before_click['close']['shadow_hit_inside_close']
                            and before_click['close']['x'] == before_move['close']['x']
                            and before_click['close']['y'] == before_move['close']['y']
                            and before_click['close']['width'] == before_move['close']['width']
                            and before_click['close']['height'] == before_move['close']['height']):
                        ActionChains(driver).click().perform()
                        break
                    stable_close = wait(stable_close_state, 'Top-level close button moved while positioning the real mouse', 15)
                result['shortcut_close'] = {
                    'aggregated_media': aggregated,
                    'stable_bounds': stable_close,
                    'attempts': click_attempts,
                }
                try:
                    wait(lambda: driver.execute_script("""
                        return ![...document.querySelectorAll('hls-downloader-media-panel')]
                            .some(host=>{const e=host.shadowRoot?.querySelector('.wrap');if(!e)return false;
                                const s=getComputedStyle(e);return s.display!=='none'&&s.visibility!=='hidden'&&e.getClientRects().length>0});
                    """), 'Top-level shortcut panel did not close after real click', 10)
                except TimeoutError:
                    result['shortcut_close'].update({
                        'after': close_state(),
                        'events': driver.execute_script('return window.__hlsShortcutCloseEvents||[]'),
                    })
                    raise RuntimeError(f'Top-level shortcut panel did not close after real click: {result["shortcut_close"]}')
                result['shortcut_close'].update({
                    'after': close_state(),
                    'events': driver.execute_script('return window.__hlsShortcutCloseEvents||[]'),
                })
                if not find_video():
                    raise RuntimeError('Player iframe was not restored after closing top-level shortcut panel')
                if args.browser == 'edge':
                    result['shortcut_attempt'].update({'iframe_panel_visible': iframe_panel_open, 'closed_by_real_click': True})
                record('actual Ctrl+Shift+Y opens one top-level panel, leaves iframe closed, and closes by real click', {
                    'top_level_visible_panels': top_panel_count,
                    'iframe_panel_visible': iframe_panel_open,
                    'closed_by_real_click': True,
                })

            if not args.popup_only:
                measure_stable(lambda: overlay(".video-download"), "public video download")
                measure_stable(lambda: overlay(".video-more"), "public video more")
                ActionChains(driver).move_to_element(overlay(".video-download")).perform()
                time.sleep(.3)
                hover = styles(overlay(".video-hover"))
                viewport = driver.execute_script("return innerWidth")
                assert hover["width"] > 0 and hover["x"] >= 0 and hover["x"]+hover["width"] <=viewport
                driver.save_screenshot(str(evidence/"public-video-hover.png"))
                record("real webpage hover card visible and inside viewport", hover)
                overlay(".video-download").click()
                click_download_confirmation(evidence_path=evidence/'page-confirmation.json', runtime=args.runtime)
                def downloaded():
                    task = next((t for t in core({"type":"snapshot","request_id":3})["tasks"]
                                 if t["task_id"] not in original_tasks and t["url"] == MEDIA), None)
                    if task:
                        created.add(task["task_id"])
                        return task if task["status"] in ("completed", "failed") else None
                task = wait(downloaded, "Actual public webpage download failed", 60)
                assert task["status"] == "completed", task
                file = Path(task["output_path"])
                assert file.stat().st_size == video_path.stat().st_size and hashlib.sha256(file.read_bytes()).hexdigest() == expected_sha256
                record("local overlay click -> actual Native Host/Core -> completed and SHA256", {"bytes":file.stat().st_size,"sha256":expected_sha256})

                overlay(".video-more").click()
                wait(lambda: overlay(".wrap") and overlay(".wrap").is_displayed(), "More button did not expand resource panel", 10)
                overlay(".pin").click()
                assert "已固定" in overlay(".pin").text
                for selector in (".pin", ".close"):
                    measure(overlay(selector), "expanded overlay " + selector)
                overlay(".pin").click()
                overlay(".close").click()
                record("overlay more, pin/unpin and collapse buttons")
                driver.save_screenshot(str(evidence/"public-video-collapsed.png"))

                # 真实拖动不能触发下载，靠近窗口边缘时悬浮卡仍应完整可见。
                count_before_drag = len(core({"type":"snapshot","request_id":31})["tasks"])
                for edge in ("top-left", "bottom-right"):
                    viewports = drag_viewports()
                    width, height = viewports['child']['width'], viewports['child']['height']
                    visible = viewports['frame']['visibleChild']
                    visible_width = visible['right'] - visible['left']
                    visible_height = visible['bottom'] - visible['top']
                    inset_x, inset_y = max(16, round(visible_width * .08)), max(16, round(visible_height * .08))
                    assert visible_width > inset_x * 2 and visible_height > inset_y * 2, viewports
                    local_x, local_y = (
                        (round(visible['left']) + inset_x, round(visible['top']) + inset_y)
                        if edge == "top-left" else
                        (round(visible['right']) - inset_x, round(visible['bottom']) - inset_y)
                    )
                    button = styles(overlay(".video-download"))
                    group_before = styles(overlay(".video-action-group"))
                    start_x = round(button["x"] + button["width"] / 2)
                    start_y = round(button["y"] + button["height"] / 2)
                    if args.browser == 'edge':
                        frame = viewports['frame']
                        w3c_start = (round(frame['left'] + start_x * frame['scaleX']),
                                     round(frame['top'] + start_y * frame['scaleY']))
                        w3c_target = (round(frame['left'] + local_x * frame['scaleX']),
                                      round(frame['top'] + local_y * frame['scaleY']))
                        action_viewport = viewports['top']
                    else:
                        w3c_start = (start_x, start_y)
                        w3c_target = (local_x, local_y)
                        action_viewport = viewports['child']
                    assert all(
                        0 <= point[0] < action_viewport['width'] and 0 <= point[1] < action_viewport['height']
                        for point in (w3c_start, w3c_target)
                    ), {
                        'viewports':viewports,'w3c_start':w3c_start,
                        'w3c_target':w3c_target,'action_viewport':action_viewport,
                    }
                    chain = ActionChains(driver)
                    chain.w3c_actions.pointer_action.move_to_location(*w3c_start)
                    chain.click_and_hold()
                    chain.w3c_actions.pointer_action.move_to_location(*w3c_target)
                    chain.release().perform()
                    time.sleep(.6)
                    group = styles(overlay(".video-action-group"))
                    moved = abs(group["x"] - group_before["x"]) + abs(group["y"] - group_before["y"])
                    assert moved > 8, {"before":group_before,"after":group,"edge":edge}
                    assert group["x"] >= 0 and group["y"] >= 0
                    assert group["x"]+group["width"] <= width and group["y"]+group["height"] <= height, group
                    ActionChains(driver).move_to_element(overlay(".video-download")).perform()
                    time.sleep(.35)
                    card = styles(overlay(".video-hover"))
                    assert card["x"] >= 0 and card["y"] >= 0 and card["x"]+card["width"] <= width and card["y"]+card["height"] <= height, card
                    driver.save_screenshot(str(evidence/("drag-"+edge+".png")))
                    record("real drag and hover card at " + edge, {
                        "viewports":viewports,"action_viewport":action_viewport,
                        "w3c_start":{"x":w3c_start[0],"y":w3c_start[1]},
                        "w3c_target":{"x":w3c_target[0],"y":w3c_target[1]},
                        "iframe_local_target":{"x":local_x,"y":local_y},
                        "group_before":group_before,"group_after":group,"card":card,
                    })
                assert len(core({"type":"snapshot","request_id":32})["tasks"]) == count_before_drag
                _fit_browser_window(driver, 720, 650)
                time.sleep(.7)
                ActionChains(driver).move_to_element(overlay(".video-download")).perform()
                time.sleep(.35)
                width, height = driver.execute_script("return [innerWidth,innerHeight]")
                group = styles(overlay(".video-action-group"))
                card = styles(overlay(".video-hover"))
                assert group["x"]+group["width"] <= width and card["x"]+card["width"] <= width and card["y"]+card["height"] <= height
                record("resize repositions dragged controls and hover card without unintended download")
                _fit_browser_window(driver)
                for zoom in (1.0, 1.5, 2.0):
                    zoom_page(zoom)
                    zoom_action('.video-download', f'{round(zoom*100)}% download hover')
                    time.sleep(.35)
                    width,height=driver.execute_script('return [innerWidth,innerHeight]')
                    group=styles(overlay('.video-action-group'))
                    card=styles(overlay('.video-hover'))
                    for rect in (group,card):
                        assert rect['x']>=0 and rect['y']>=0 and rect['x']+rect['width']<=width+1 and rect['y']+rect['height']<=height+1,rect
                    zoom_action('.video-more', f'{round(zoom*100)}% more click', click=True)
                    wait(lambda:overlay('.wrap').is_displayed(),'Zoomed more button did not open',10)
                    zoom_action('.close', f'{round(zoom*100)}% close click', click=True)
                    record(f'actual {args.browser} page zoom {round(zoom*100)}% hover bounds and clickable controls')
                zoom_page(1)

            driver.switch_to.new_window("tab")
            popup = driver.current_window_handle
            driver.get(popup_url)
            wait(lambda: driver.execute_script("return document.documentElement.dataset.popupReady==='ready'"), "Popup not ready", 25)
            assert message({"type":"ping"}).get("ok") is True
            record("popup actually connected to installed Native Host/Core")
            for b in driver.find_elements(By.TAG_NAME,"button"):
                if b.is_displayed() and not args.actions_only: measure(b,"popup light")
            driver.save_screenshot(str(evidence/"popup-light.png"))

            theme = driver.find_element(By.CSS_SELECTOR,".header-actions button:first-child")
            def click_theme():
                driver.execute_script(
                    "arguments[0].scrollIntoView({block:'center',inline:'nearest'})", theme
                )
                ActionChains(driver).move_to_element(theme).click().perform()

            for _ in range(3):
                old = theme.get_attribute("title")
                click_theme()
                wait(lambda: theme.get_attribute("title") != old, "Theme did not change", 5)
            record("popup theme button cycles through automatic/light/dark")
            # 使用实际主题按钮，直到进入深色态，再测交互样式。
            for _ in range(3):
                if driver.execute_script("return document.documentElement.dataset.hlsdTheme") == "dark": break
                click_theme()
                time.sleep(.2)
            assert driver.execute_script("return document.documentElement.dataset.hlsdTheme") == "dark"
            for b in driver.find_elements(By.TAG_NAME,"button"):
                if b.is_displayed() and not args.actions_only: measure(b,"popup dark")

            popup_button("设置").click()
            for label in ("自动接管", "本站 Cookie", "本站提示"):
                b = popup_button(label)
                old = b.get_attribute("aria-pressed")
                b.click()
                wait(lambda: popup_button(label).get_attribute("aria-pressed") != old, "Popup toggle did not change: " + label, 10)
                popup_button(label).click()
                wait(lambda: popup_button(label).get_attribute("aria-pressed") == old, "Popup toggle did not restore: " + label, 10)
                record("popup toggle and restore: " + label)
            popup_button("资源").click()
            popup_button("重新识别").click()
            wait(lambda: driver.execute_script("return !document.querySelector('.scan-button').disabled"), "Rescan did not finish", 20)
            wait(lambda: driver.find_elements(By.CSS_SELECTOR,"article .copy-link"), "Rescan lost local video", 20)
            copy = driver.find_element(By.CSS_SELECTOR,"article .copy-link")
            driver.execute_script("arguments[0].scrollIntoView({block:'center'})", copy)
            copy.click()
            wait(lambda: "已复制" in driver.find_element(By.CSS_SELECTOR,"article .copy-link").text, "Copy link did not confirm success", 5)
            copy.click()
            assert copy.find_elements(By.CSS_SELECTOR, '.hlsd-icon svg'), 'Repeated copy removed the icon'
            wait(lambda: copy.find_element(By.CSS_SELECTOR, '.copy-label').text == '复制链接', 'Copy label did not reset', 5)
            record("popup rescan and copy-link buttons")

            if args.partitioned_cookies:
                popup_button('设置').click()
                if popup_button('本站 Cookie').get_attribute('aria-pressed') != 'true':
                    popup_button('本站 Cookie').click()
                    wait(lambda: popup_button('本站 Cookie').get_attribute('aria-pressed') == 'true', 'Cookie authorization did not apply', 10)
                seeded = driver.execute_async_script('''
                    const [page,done]=arguments,api=globalThis.browser||chrome;
                    (async()=>{
                        const tab=(await api.tabs.query({})).find(t=>t.url===page);
                        const origin=new URL(page).origin;
                        const key=api.cookies.getPartitionKey ? (await api.cookies.getPartitionKey({tabId:tab.id,frameId:0})).partitionKey
                            : {topLevelSite:origin};
                        const base={url:origin+'/',path:'/',name:'hlsqa_partition',secure:true,sameSite:'no_restriction'};
                        const results=[];
                        results.push(await api.cookies.set({...base,value:'ordinary'}));
                        results.push(await api.cookies.set({...base,value:'partitioned',partitionKey:key}));
                        results.push(await api.cookies.set({...base,value:'foreign',partitionKey:{...key,topLevelSite:'https://example.org',hasCrossSiteAncestor:true}}));
                        return {key,values:results.map(c=>({value:c.value,partitionKey:c.partitionKey}))};
                    })().then(done,e=>done({error:String(e)}));
                ''', PAGE)
                assert not seeded.get('error'), seeded
                record('real browser ordinary, current-site partitioned and foreign-partition cookies seeded', seeded)
                popup_button('资源').click()

            before_popup_download = {t["task_id"] for t in core({"type":"snapshot","request_id":41})["tasks"]}
            before_handoffs = {json.loads(value)['id'] for value in core({'type':'load_handoffs','request_id':43})['items']} if args.partitioned_cookies else set()
            card = next(a for a in driver.find_elements(By.TAG_NAME,"article") if "stream.mp4" in a.text)
            download_button = card.find_element(By.CSS_SELECTOR,'button[data-action="download"]')
            driver.execute_script("arguments[0].scrollIntoView({block:'center'})",download_button)
            ActionChains(driver).click(download_button).perform()
            if args.partitioned_cookies:
                def offered():
                    return next((item for value in core({'type':'load_handoffs','request_id':44})['items']
                                 if (item:=json.loads(value))['id'] not in before_handoffs and item['offer']['url']==MEDIA),None)
                handoff=wait(offered,'Partitioned cookie handoff not received by installed Core',15)
                test_handoff_id=handoff['id']
                protected=core({'type':'load_credential','request_id':45,'credential_ref':handoff['offer']['credential_ref']})['protected_blob']
                context=test_replay_context(protected)
                cookies=context['request_contexts'][media_origin]['cookies']
                selected=[c for c in cookies if c['name']=='hlsqa_partition']
                assert sorted(c['value'] for c in selected)==['ordinary','partitioned'],selected
                assert 'hlsqa_partition=ordinary' in context['cookie'] and 'hlsqa_partition=partitioned' in context['cookie'],context['cookie']
                assert 'hlsqa_partition=foreign' not in context['cookie'],context['cookie']
                record('installed Native Host/Core preserves both same-name cookies and excludes foreign partition', selected)
            click_download_confirmation(evidence_path=evidence/'popup-confirmation.json', runtime=args.runtime)
            def popup_downloaded():
                task=next((t for t in core({"type":"snapshot","request_id":42})["tasks"] if t["task_id"] not in before_popup_download and t["url"]==MEDIA),None)
                if task:
                    created.add(task["task_id"])
                    return task if task["status"] in ("completed","failed") else None
            task=wait(popup_downloaded,"Popup download did not complete",60)
            assert task["status"]=="completed" and hashlib.sha256(Path(task["output_path"]).read_bytes()).hexdigest()==expected_sha256,task
            record("popup download button -> actual local video -> completed and SHA256")
            queued = wait(lambda: driver.find_elements(By.CSS_SELECTOR, '[data-action="download"][data-state="queued"]'),
                          'Download button did not show confirmed state', 10)[0]
            assert queued.get_attribute('aria-busy') == 'false' and not queued.is_enabled()
            assert not driver.execute_script('return arguments[0].classList.contains("busy")', queued)
            record('confirmed download has a stable check icon and no busy animation')

            # 此处只验证字幕设置交互；真实云端音频另行验收。
            popup_button("字幕").click()
            for label, value in (("来源语言", "en"), ("目标语言", "ja"), ("本站字幕翻译", "off"), ("本页字幕翻译", "on")):
                select = driver.find_element(By.CSS_SELECTOR,f'select[aria-label="{label}"]')
                original = select.get_attribute("value")
                Select(select).select_by_value(value)
                wait(lambda: driver.find_element(By.CSS_SELECTOR,f'select[aria-label="{label}"]').get_attribute("value")==value,
                     "Subtitle choice did not apply: "+label, 10)
                Select(driver.find_element(By.CSS_SELECTOR,f'select[aria-label="{label}"]')).select_by_value(original)
                record("subtitle popup select and restore: "+label)
            driver.find_element(By.CSS_SELECTOR, ".subtitle-panel summary").click()
            for b in driver.find_elements(By.CSS_SELECTOR, ".subtitle-provider button"):
                if not args.actions_only: measure(b, "subtitle provider")
            service = driver.find_element(By.CSS_SELECTOR, 'select[aria-label="翻译服务"]')
            original_service = service.get_attribute('value')
            workspace = driver.find_element(By.CSS_SELECTOR, 'input[aria-label="百炼业务空间 ID"]')
            original_workspace = workspace.get_attribute('value')
            Select(service).select_by_value('dashscope')
            workspace.clear()
            save_button = popup_button("保存服务设置")
            save_button.click()
            assert not driver.execute_script("return document.querySelector('.subtitle-provider').checkValidity()")
            driver.execute_script('arguments[0].value=arguments[1]', workspace, original_workspace)
            Select(service).select_by_value(original_service)
            record("DashScope without workspace: browser validation prevents invalid save and original service is restored")
            popup_button("主程序").click()
            record("popup open desktop button")
            driver.save_screenshot(str(evidence/"popup-final.png"))
            close_button=driver.find_element(By.CSS_SELECTOR,'button[title="关闭"]')
            ActionChains(driver).click(close_button).perform()
            wait(lambda: popup not in driver.window_handles,"Popup close button did not close window",10)
            driver.switch_to.window(player)
            record("popup close button closes its window and keeps webpage open")
            result["passed"] = True
        except Exception as error:
            result["error"] = str(error)
            result["traceback"] = traceback.format_exc()
            if driver:
                try:
                    result['ui_diagnostics'] = driver.execute_script('''
                        const visible = element => {
                          const style = getComputedStyle(element);
                          return style.display !== 'none' && style.visibility !== 'hidden' && element.getClientRects().length > 0;
                        };
                        return {
                          url: location.href,
                          text: (document.body?.innerText || '').slice(0, 8000),
                          errors: [...document.querySelectorAll('.send-error,.error,[role="alert"]')]
                            .filter(visible).map(element => element.innerText || element.textContent || '').filter(Boolean),
                          pageErrors: window.__hlsUiErrors,
                          pointers: window.__hlsPointerEvents,
                        };
                    ''')
                    result['popup_failure_screenshot'] = str(evidence/"failure-popup.png")
                    driver.save_screenshot(result['popup_failure_screenshot'])
                    driver.save_screenshot(str(evidence/"failure.png"))
                except Exception as diagnostic_error:
                    result['ui_diagnostics_error'] = str(diagnostic_error)
                if args.partitioned_cookies:
                    try:
                        if args.browser == 'edge':
                            result['cookie_frame_diagnostics'] = driver.execute_async_script('''
                                const [page,done]=arguments,api=globalThis.browser||chrome;
                                (async()=>{
                                    const tab=(await api.tabs.query({})).find(t=>t.url===page);
                                    const resources=await api.runtime.sendMessage({type:'list',pageUrl:page,tabId:tab.id});
                                    const frames=[...new Set([0,...resources.map(r=>r.frameId??0)])];
                                    return {tabId:tab.id,resources:resources.map(r=>({url:new URL(r.url).origin+new URL(r.url).pathname,frameId:r.frameId,kind:r.kind})),
                                        frames:await Promise.all(frames.map(async frameId=>{
                                            const entry={frameId};
                                            try{entry.key=(await api.cookies.getPartitionKey({tabId:tab.id,frameId})).partitionKey}
                                            catch(e){entry.error=String(e);entry.message=e?.message;entry.isError=e instanceof Error}
                                            try{entry.context=await api.tabs.sendMessage(tab.id,{type:'srcdoc-partition-context'},{frameId})}
                                            catch(e){entry.contextError=String(e)}
                                            return entry;
                                        }))};
                                })().then(done,e=>done({error:String(e)}));
                            ''', PAGE)
                        expected = urlsplit(MEDIA)
                        summaries = []
                        for value in core({'type':'load_handoffs','request_id':49})['items']:
                            handoff = json.loads(value)
                            offer = handoff.get('offer') or {}
                            raw_url = str(offer.get('url') or '')
                            parsed = urlsplit(raw_url)
                            if (parsed.scheme, parsed.netloc, parsed.path) != (expected.scheme, expected.netloc, expected.path):
                                continue
                            safe_url = urlunsplit((parsed.scheme, parsed.netloc, parsed.path,
                                                   urlencode([(key, '<redacted>') for key, _ in parse_qsl(parsed.query, keep_blank_values=True)]), ''))
                            summaries.append({
                                'id': handoff.get('id'), 'status': handoff.get('status'), 'url': safe_url,
                                'new_since_click_baseline': handoff.get('id') not in before_handoffs,
                            })
                        result['media_handoffs'] = summaries
                    except Exception as diagnostic_error:
                        result['media_handoffs_error'] = str(diagnostic_error)
            print(json.dumps({"error":str(error)},ensure_ascii=True),flush=True)
        finally:
            if driver:
                try:
                    driver.quit()
                except Exception as cleanup_error:
                    result['driver_quit_error'] = str(cleanup_error)
            if fixture_certificate_cleanup_needed:
                try:
                    store_result = run_certutil(
                        ['-f', '-delstore', 'Root', fixture_certificate_thumbprint]
                    )
                    result['fixture_certificate_store'].update({
                        'delete_exit_code': store_result['exit_code'],
                        'delete_stdout': store_result['stdout'],
                        'delete_stderr': store_result['stderr'],
                        'delete_timed_out': store_result['timed_out'],
                    })
                    delete_failed = store_result['timed_out'] or store_result['exit_code'] != 0
                except OSError as error:
                    result['fixture_certificate_store']['delete_error'] = str(error)
                    delete_failed = True
                if delete_failed:
                    result['passed'] = False
                    result['certificate_cleanup_error'] = (
                        'Could not remove the temporary fixture certificate'
                    )
            if video_server:
                if video_server_started:
                    with contextlib.suppress(Exception):
                        video_server.shutdown()
                with contextlib.suppress(Exception):
                    video_server.server_close()
            if test_handoff_id:
                for value in core({'type':'load_handoffs','request_id':46})['items']:
                    handoff=json.loads(value)
                    if handoff['id']==test_handoff_id and handoff['status']=='pending':
                        core({'type':'command','request_id':47,'command':{'kind':'reject_handoff','handoff_id':test_handoff_id,'suppress_site_kind':False}})
            for task_id in created:
                core({"type":"command","request_id":7,"command":{"kind":"task_action","task_id":task_id,"action":"delete_files"}})
            if original_settings is not None:
                core({"type":"store_settings","request_id":8,"values":{
                    "browser_takeover_enabled":original_settings["takeover_enabled"],
                    "browser_takeover_minimum_bytes":original_settings["takeover_minimum_bytes"]}})
            result['original_tasks_preserved']=original_tasks <= {t['task_id'] for t in core({'type':'snapshot','request_id':48})['tasks']}
            args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding="utf8")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument('--browser', choices=['firefox', 'edge'], default='firefox')
    p.add_argument('--headed', action='store_true')
    p.add_argument("--popup-only", action="store_true")
    p.add_argument("--actions-only", action="store_true")
    p.add_argument("--shortcut", action="store_true")
    p.add_argument("--partitioned-cookies", action="store_true")
    p.add_argument("--runtime",type=Path,required=True)
    p.add_argument("--extension",type=Path,required=True)
    p.add_argument("--driver",type=Path,required=True)
    p.add_argument("--report",type=Path,required=True)
    raise SystemExit(run(p.parse_args()))
