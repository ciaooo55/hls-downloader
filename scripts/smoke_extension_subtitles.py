"""Production browser UI/audio smoke with the real Native Host and a fixture Core.

The provider is simulated deliberately; cloud speech accuracy is not asserted.
Browser profiles, Core TCP endpoint and registry registration are isolated and
the previous Native Messaging registration is restored in finally.
"""
from __future__ import annotations

import argparse
import base64
import contextlib
import functools
import http.server
import io
import json
import math
import os
import re
from pathlib import Path
import socketserver
import struct
import subprocess
import tempfile
import threading
import time
import wave
from urllib.request import urlopen

import websocket

from smoke_extension_browsers import (
    _chromium_extension_id, _evaluate, _find_chromium_binary, _free_port,
    _open_debug_target, _read_debug_targets, _capture_screenshot, _stop_process_tree,
    _zip_firefox_extension,
)
from smoke_extension_takeover import _registered_host


class FixtureCore(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self):
        super().__init__(("127.0.0.1", 0), CoreHandler)
        self.sessions = {}
        self.lock = threading.Lock()
        self.audio_frames = 0
        self.nonzero_samples = 0
        self.resets = 0
        self.fail_poll = False
        self.fail_reset = False
        self.prefetch_positions = []

    def respond(self, request):
        kind = request["type"]
        rid = request.get("request_id", 1)
        if kind == "hello":
            return {"type":"hello", "protocol":"hls-downloader-v7-core", "version":1, "pid":os.getpid()}
        if kind == "load_handoffs": return {"type":"handoffs", "request_id":rid, "items":[]}
        if kind == "load_settings":
            return {"type":"settings", "request_id":rid, "takeover_enabled":True, "takeover_minimum_bytes":0, "legal_accepted":True, "speed_limit_kib":0, "harvest_minimum_bytes":0}
        if kind == "command": return {"type":"events", "request_id":rid, "events":[]}
        if kind != "subtitle": return {"type":"error", "request_id":rid, "code":"fixture", "message":"unsupported fixture request"}
        request = request["request"]
        action = request["action"]
        sid = request.get("session_id", "")
        result = {"ok":True}
        with self.lock:
            if action == "configuration": result.update(workspace="fixture-only", region="cn-beijing", has_key=True)
            elif action == "start":
                assert request["source"] == "auto" and request["target"] == "zh"
                self.fail_poll = False
                self.fail_reset = False
                self.sessions[sid] = {"status":"running", "epoch":request["epoch"], "events":[], "chunks":0}
                result["status"] = "running"
            elif action == "stop":
                if sid in self.sessions: self.sessions[sid]["status"] = "stopped"
            elif sid not in self.sessions: result = {"ok":False, "error":"fixture session missing"}
            elif action == "reset":
                if self.fail_reset:
                    result = {"ok":False, "error":"测试快进重置失败"}
                else:
                    session = self.sessions[sid]
                    session.update(epoch=request["epoch"], chunks=0)
                    self.resets += 1
            elif action == "audio":
                session = self.sessions[sid]
                pcm = base64.b64decode(request["audio"], validate=True)
                assert len(pcm) == 8000
                assert request["epoch"] == session["epoch"]
                samples = struct.unpack("<4000h", pcm)
                self.audio_frames += 1
                self.prefetch_positions.append(request['position'])
                self.nonzero_samples += sum(sample != 0 for sample in samples)
                session["chunks"] += 1
                if session["chunks"] % 4 == 0:
                    start = request["position"]
                    for role, text in (("translation","浏览器字幕链路验证"),("source","Browser audio pipeline")):
                        session["events"].append({"type":"cue", "id":f"speech-{session['epoch']}-{session['chunks']}", "role":role,
                            "text":text, "final":True, "start":start, "end":start+3,
                            "epoch":session["epoch"], "sequence":len(session["events"])+1})
            elif action == "poll":
                session = self.sessions[sid]
                if self.fail_poll: session.update(status="failed", error="测试服务断开")
                result.update(status=session["status"], epoch=session["epoch"], latest_sequence=len(session["events"]),
                    error=session.get("error",""), events=[e for e in session["events"] if e["sequence"] > request.get("after_sequence",0)])
        return {"type":"subtitle", "request_id":rid, "result":result}


class CoreHandler(socketserver.StreamRequestHandler):
    def handle(self):
        with contextlib.suppress(OSError): self.frames()

    def frames(self):
        self.connection.settimeout(30)
        while True:
            header = self.rfile.read(4)
            if len(header) != 4: return
            length = struct.unpack("<I", header)[0]
            if length > 4*1024*1024: return
            body = self.rfile.read(length)
            if len(body) != length: return
            response = self.server.respond(json.loads(body))
            encoded = json.dumps(response, ensure_ascii=False).encode("utf-8")
            self.wfile.write(struct.pack("<I", len(encoded))+encoded)
            self.wfile.flush()


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_args): pass

    def do_GET(self):
        match = re.fullmatch(r'bytes=(\d+)-(\d*)', self.headers.get('Range', ''))
        path = Path(self.translate_path(self.path))
        if not match or not path.is_file():
            return super().do_GET()
        total = path.stat().st_size
        start = int(match[1]); end = min(int(match[2]) if match[2] else total-1, total-1)
        if start > end:
            self.send_response(416); self.send_header('Content-Range',f'bytes */{total}'); self.end_headers(); return
        self.send_response(206); self.send_header('Accept-Ranges','bytes')
        self.send_header('Content-Range',f'bytes {start}-{end}/{total}')
        self.send_header('Content-Length',str(end-start+1)); self.send_header('Content-Type',self.guess_type(str(path)))
        self.end_headers()
        with path.open('rb') as stream:
            stream.seek(start)
            with contextlib.suppress(BrokenPipeError, ConnectionResetError): self.wfile.write(stream.read(end-start+1))


def wait(predicate, description, timeout=25):
    deadline = time.monotonic()+timeout
    while time.monotonic() < deadline:
        with contextlib.suppress(Exception):
            result = predicate()
            if result: return result
        time.sleep(.1)
    raise AssertionError(description)


def trigger_action(port, extension_id, page_url):
    with urlopen(f"http://127.0.0.1:{port}/json/version", timeout=2) as response:
        browser_socket = json.load(response)["webSocketDebuggerUrl"]
    connection = websocket.create_connection(browser_socket, timeout=5, suppress_origin=True)
    try:
        connection.send(json.dumps({"id":1, "method":"Target.getTargets", "params":{"filter":[{"type":"tab","exclude":False}]}}))
        while True:
            response = json.loads(connection.recv())
            if response.get("id") == 1:
                target_id = next(t['targetId'] for t in response['result']['targetInfos'] if t.get('url') == page_url)
                break
        connection.send(json.dumps({"id":2, "method":"Extensions.triggerAction", "params":{"id":extension_id,"targetId":target_id}}))
        while True:
            response = json.loads(connection.recv())
            if response.get("id") == 2:
                if "error" in response: raise RuntimeError(f"extension toolbar invocation failed: {response['error']}")
                return
    finally:
        connection.close()


def run(args):
    args.report.parent.mkdir(parents=True, exist_ok=True)
    extension = args.extension.resolve()
    firefox = args.browser == 'firefox'
    eid = json.loads((extension/'manifest.json').read_text(encoding='utf-8'))['browser_specific_settings']['gecko']['id'] if firefox else _chromium_extension_id(extension)
    browser = Path(args.browser_binary).resolve() if firefox else _find_chromium_binary(args.browser_binary)
    family = "firefox" if firefox else "edge" if "msedge" in browser.name.lower() else "chrome"
    if firefox and args.capture == 'tab': raise ValueError('Firefox cannot use tab capture')
    if args.prefetch_fallback and args.capture != 'tab': raise ValueError('fallback fixture requires cross-origin tab capture')
    temp_root = Path(tempfile.gettempdir()) if args.system_temp else Path(__file__).resolve().parents[1]/'.tool-cache'/'test-tmp'
    temp_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="hls-subtitle-smoke-", dir=temp_root) as temporary:
        root = Path(temporary)
        site = root/"site"; site.mkdir()
        audio = io.BytesIO()
        with wave.open(audio,"wb") as wav:
            wav.setnchannels(1); wav.setsampwidth(2); wav.setframerate(16000)
            duration = 70 if args.capture == 'prefetch' else 10
            wav.writeframes(b"".join(struct.pack("<h",int(math.sin(i*math.tau*440/16000)*5000)) for i in range(duration*16000)))
        (site/"tone.wav").write_bytes(audio.getvalue())
        origin = http.server.ThreadingHTTPServer(("127.0.0.1",0),functools.partial(QuietHandler,directory=str(site)))
        audio_url = f"http://localhost:{origin.server_port}/tone.wav" if args.capture == 'tab' else 'tone.wav'
        (site/"player.html").write_text(f'''<!doctype html><meta charset="utf-8"><title>字幕测试播放器</title><div id="player"></div><script>
        const mode={json.dumps(args.player_dom)};
        const mount=()=>{{
          let root=document.querySelector('#player');
          if(mode!=='document') root=root.attachShadow({{mode:'open'}});
          if(mode==='nested-shadow') {{ const host=document.createElement('section');root.append(host);root=host.attachShadow({{mode:'open'}}); }}
          window.testMedia=document.createElement('video');
          testMedia.src={json.dumps(audio_url)};testMedia.controls=true;testMedia.autoplay=true;testMedia.loop=true;
          testMedia.style='width:640px;height:360px';root.append(testMedia);
        }};
        if(mode==='dynamic-shadow') setTimeout(mount,1200);else mount();
        </script>''',encoding="utf-8")
        origin_thread = threading.Thread(target=origin.serve_forever,daemon=True); origin_thread.start()
        core = FixtureCore(); core_thread = threading.Thread(target=core.serve_forever,daemon=True); core_thread.start()
        manifest = root/"native-host.json"
        manifest.write_text(json.dumps({"name":"com.ciaooo55.hls_downloader", "description":"isolated subtitle smoke",
            "path":str(args.host.resolve()), "type":"stdio", **({'allowed_extensions':[eid]} if firefox else {'allowed_origins':[f"chrome-extension://{eid}/"]})}),encoding="utf-8")
        port = _free_port()
        env = os.environ.copy(); env.update(HLS_V7_PIPE=rf"\\.\pipe\HLSSubtitleSmoke-{os.getpid()}",HLS_V7_CORE_TCP="1",HLS_V7_CORE_BIND=f"127.0.0.1:{core.server_address[1]}")
        process = None
        driver = None
        def evaluate(target, expression):
            if driver is None: return _evaluate(target, expression)
            driver.switch_to.window(target)
            result = driver.execute_async_script('const done=arguments[arguments.length-1]; Promise.resolve('+expression+').then(done,error=>done({__error:String(error)}));')
            if isinstance(result, dict) and '__error' in result: raise RuntimeError(result['__error'])
            return result
        def screenshot(target, destination):
            if driver is None: return _capture_screenshot(target, destination)
            driver.switch_to.window(target); driver.save_screenshot(str(destination))
        try:
            with _registered_host(manifest,family):
                page_url = f"http://127.0.0.1:{origin.server_port}/player.html"
                if firefox:
                    from selenium import webdriver
                    from selenium.webdriver.firefox.options import Options
                    from selenium.webdriver.firefox.service import Service
                    addon = root/'subtitle-smoke.xpi'; _zip_firefox_extension(extension, addon)
                    options = Options(); options.binary_location = str(browser); options.add_argument('-headless')
                    options.set_preference('media.autoplay.default', 0)
                    options.set_preference('media.autoplay.blocking_policy', 0)
                    service = Service(executable_path=args.firefox_driver, env=env,
                        service_args=['--profile-root',str(root),'--allow-system-access'],
                        log_output=str(args.report.with_suffix('.gecko.log')))
                    driver = webdriver.Firefox(service=service, options=options)
                    driver.set_page_load_timeout(20); driver.set_script_timeout(10)
                    assert driver.install_addon(str(addon), temporary=True) == eid
                    driver.set_context('chrome')
                    uuids = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
                    driver.set_context('content')
                    driver.get(page_url); page = driver.current_window_handle
                    wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('button')?.textContent === '字幕翻译已关闭'"),'subtitle entry missing')
                    driver.execute_async_script("const done=arguments[arguments.length-1]; window.testMedia.play().then(()=>done(true),e=>done(String(e)))")
                    driver.switch_to.new_window('tab'); inspector = driver.current_window_handle
                    popup_url = f"moz-extension://{uuids[eid]}/popup.html?inspector=1"
                    driver.get(popup_url)
                    driver.switch_to.window(page)
                    evaluate(inspector,f"browser.tabs.query({{}}).then(t=>browser.tabs.update(t.find(t=>t.url==={json.dumps(page_url)}).id,{{active:true}})).then(()=>browser.tabs.create({{url:browser.runtime.getURL('popup.html?site=1'),active:false}}))")
                    popup = wait(lambda:next((h for h in driver.window_handles if h not in [page,inspector]),None),'popup missing')
                else:
                    process = subprocess.Popen([str(browser),"--headless=new",f"--remote-debugging-port={port}","--remote-allow-origins=*",
                    f"--user-data-dir={root/'profile'}","--no-first-run","--no-default-browser-check","--autoplay-policy=no-user-gesture-required",
                    "--enable-unsafe-extension-debugging","--disable-features=DisableLoadExtensionCommandLineSwitch",f"--disable-extensions-except={extension}",f"--load-extension={extension}","about:blank"],
                    env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,creationflags=getattr(subprocess,"CREATE_NO_WINDOW",0))
                    wait(lambda:_read_debug_targets(port),"browser startup failed")
                    _open_debug_target(port,page_url)
                    page_target = wait(lambda:next((t for t in _read_debug_targets(port) if t.get('url')==page_url),None),"web page missing")
                    page = page_target["webSocketDebuggerUrl"]
                    wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('button')?.textContent === '字幕翻译已关闭'"),"subtitle entry missing")
                    popup_url = f"chrome-extension://{eid}/popup.html"
                    if args.capture == 'tab': trigger_action(port, eid, page_url)
                    # 工具栏弹窗会因焦点变化销毁；仍实际调用工具栏授予采集权限，再用持久标签测试同一 UI。
                    previous_targets = {t['id'] for t in _read_debug_targets(port)}
                    _open_debug_target(port,popup_url)
                    popup = wait(lambda:next((t for t in _read_debug_targets(port) if t.get('url')==popup_url and t['id'] not in previous_targets),None),"popup missing")["webSocketDebuggerUrl"]
                wait(lambda:evaluate(popup,"Boolean(document.querySelector('[data-subtitle-panel] select option'))"),"subtitle popup missing")
                evaluate(popup,"(()=>{const c=document.querySelector('[data-subtitle-enabled]');c.checked=true;c.dispatchEvent(new Event('change'));return true})()")
                wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('button')?.disabled === false"),"global enable not broadcast")
                evaluate(popup,"(()=>{const c=document.querySelector('[aria-label=\"本站字幕翻译\"]');c.value='off';c.dispatchEvent(new Event('change'));return true})()")
                wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('button')?.disabled === true"),"site disable not applied")
                evaluate(popup,"(()=>{const c=document.querySelector('[aria-label=\"本页字幕翻译\"]');c.value='on';c.dispatchEvent(new Event('change'));return true})()")
                wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('button')?.disabled === false"),"page override not applied")
                wait(lambda:evaluate(popup,"document.querySelector('[data-subtitle-start]')?.disabled === false"),"player not selected")
                prefetch_checks = []
                if args.capture == 'prefetch' or args.prefetch_fallback:
                    evaluate(popup,"(()=>{const c=document.querySelector('[data-subtitle-prefetch]');c.checked=true;c.dispatchEvent(new Event('change'));return true})()")
                    wait(lambda:evaluate(popup,"chrome.storage.local.get('subtitle-settings').then(v=>v['subtitle-settings']?.prefetch === true)"),"prefetch setting not saved")
                    if args.capture == 'prefetch':
                        evaluate(page,"(()=>{const v=window.testMedia;v.pause();v.currentTime=0;return true})()")
                    wait(lambda:evaluate(popup,"document.querySelector('[data-subtitle-start]')?.disabled === false"),"settings update did not reenable start")
                evaluate(popup,"document.querySelector('[data-subtitle-start]').click()")
                wait(lambda:core.audio_frames>=4,"tab audio was not uploaded: "+str(evaluate(popup,"document.querySelector('.subtitle-error')?.textContent")))
                capture = evaluate(popup, "chrome.storage.session.get('subtitle-sessions').then(v=>Object.values(v['subtitle-sessions']||{})[0]?.capture)")
                assert capture == args.capture, f"expected {args.capture} capture, got {capture}"
                wait(lambda:core.nonzero_samples > 1000, "captured audio remained silent")
                if args.prefetch_fallback:
                    wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('.status')?.textContent.includes('实时采集：')"),"unreadable audio did not disclose fallback")
                    prefetch_checks.append('CORS failure falls back to real tab capture with visible reason')
                if args.capture == 'prefetch':
                    wait(lambda:max(core.prefetch_positions,default=0)>3,"paused player did not prepare future audio")
                    frames_before_stall = core.audio_frames
                    evaluate(page,"(()=>{const end=performance.now()+1100;while(performance.now()<end){}return true})()")
                    wait(lambda:core.audio_frames>=frames_before_stall+4,"audio upload stopped during page thread stall")
                    positions = list(core.prefetch_positions)
                    assert all(abs(second-first-.25)<.02 for first,second in zip(positions,positions[1:])), f'PCM timeline jumped or repeated during page thread stall: {positions}'
                    state = evaluate(page,"(()=>{const v=window.testMedia;return {time:v.currentTime,paused:v.paused,rate:v.playbackRate,caption:document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('.translation')?.textContent}})()")
                    assert state == {'time':0,'paused':True,'rate':1,'caption':''}, f'prefetch modified player or displayed future cue: {state}'
                    wait(lambda:max(core.prefetch_positions,default=0)>=29.5,"prefetch did not reach its 30-second window",timeout=55)
                    frames = core.audio_frames
                    time.sleep(1)
                    assert core.audio_frames <= frames+1 and max(core.prefetch_positions)<=30.5, 'prefetch continued past the bounded window'
                    evaluate(page,"document.querySelector('[data-hls-subtitle]').shadowRoot.querySelector('.tools button:nth-child(3)').click()")
                    wait(lambda:evaluate(page,"window.testMedia.currentTime > 0.5"),"next cue did not seek into prepared subtitles")
                    prefetch_checks.extend(['future PCM uploaded while original player stays paused at time zero', 'PCM positions stay accurate while page thread is blocked', 'future captions hidden until their time', '30-second pretranslation window stops PCM', 'next cue seeks into prepared cache'])
                wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('.translation')?.textContent.includes('字幕链路验证')"),"translation not rendered")
                evaluate(page,"window.testMedia.currentTime=5")
                wait(lambda:core.resets>0,"seek did not reset the provider session")
                wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('.translation')?.textContent.includes('字幕链路验证')"),"post-seek translation missing")
                reset_count = core.resets
                evaluate(page,"window.testMedia.playbackRate=2")
                wait(lambda:core.resets > reset_count,"rate change did not reset provider")
                evaluate(popup,"(()=>{const c=document.querySelector('[aria-label=\"字幕时间偏移秒\"]');c.value='1.2';c.dispatchEvent(new Event('change'));return true})()")
                wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('.status')?.textContent.includes('+1.2s')"),"subtitle offset not applied")
                core.fail_poll = True
                wait(lambda:evaluate(popup,"document.querySelector('.subtitle-error')?.textContent.includes('测试服务断开')"),"provider failure not shown")
                wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('.status')?.textContent.includes('测试服务断开')"),"provider failure not shown on player")
                wait(lambda:all(s['status']=='stopped' for s in core.sessions.values()),"provider failure did not stop capture session")
                previous_sessions = len(core.sessions)
                wait(lambda:evaluate(popup,"document.querySelector('[data-subtitle-start]')?.disabled === false"),"retry button not enabled")
                evaluate(popup,"document.querySelector('[data-subtitle-start]').click()")
                wait(lambda:len(core.sessions) > previous_sessions and any(s['chunks'] >= 4 and s['status']=='running' for s in core.sessions.values()),"restart after failure did not capture audio")
                core.fail_reset = True
                evaluate(page,"window.testMedia.currentTime=1")
                wait(lambda:evaluate(popup,"document.querySelector('.subtitle-error')?.textContent.includes('测试快进重置失败')"),"reset rejection not shown")
                wait(lambda:all(s['status']=='stopped' for s in core.sessions.values()),"reset rejection did not stop native session")
                previous_sessions = len(core.sessions)
                wait(lambda:evaluate(popup,"document.querySelector('[data-subtitle-start]')?.disabled === false"),"reset retry button not enabled")
                evaluate(popup,"document.querySelector('[data-subtitle-start]').click()")
                wait(lambda:len(core.sessions) > previous_sessions and any(s['chunks'] >= 4 and s['status']=='running' for s in core.sessions.values()),"retry after reset rejection did not capture audio")
                evaluate(popup,"(()=>{const c=document.querySelector('[data-subtitle-enabled]');c.checked=false;c.dispatchEvent(new Event('change'));return true})()")
                wait(lambda:all(s['status']=='stopped' for s in core.sessions.values()),"disable with cached captions did not stop translation")
                wait(lambda:evaluate(page,"(()=>{const s=document.querySelector('[data-hls-subtitle]').shadowRoot;return s.querySelector('.translation').textContent === '' && [...s.querySelectorAll('.tools button')].slice(1,3).every(b=>b.hidden && b.disabled)})()"),"disable left cached subtitle navigation active")
                previous_sessions = len(core.sessions)
                evaluate(popup,"(()=>{const c=document.querySelector('[data-subtitle-enabled]');c.checked=true;c.dispatchEvent(new Event('change'));return true})()")
                wait(lambda:evaluate(popup,"document.querySelector('[data-subtitle-start]')?.disabled === false"),"global enable did not restore start")
                evaluate(popup,"document.querySelector('[data-subtitle-start]').click()")
                wait(lambda:len(core.sessions) > previous_sessions and any(s['chunks'] >= 4 and s['status']=='running' for s in core.sessions.values()),"reenable did not restore audio")
                evaluate(popup,"(()=>{const c=document.querySelector('[aria-label=\"目标语言\"]');c.value='en';c.dispatchEvent(new Event('change'));return true})()")
                wait(lambda:all(s['status']=='stopped' for s in core.sessions.values()),"language change did not stop existing translation")
                wait(lambda:evaluate(page,"(()=>{const s=document.querySelector('[data-hls-subtitle]').shadowRoot;return s.querySelector('.translation').textContent === '' && s.querySelector('.tools button:nth-child(3)').disabled})()"),"language change retained old translated cues")
                evaluate(popup,"(()=>{const c=document.querySelector('[data-subtitle-enabled]');c.checked=false;c.dispatchEvent(new Event('change'));return true})()")
                wait(lambda:all(s['status']=='stopped' for s in core.sessions.values()),"global disable did not stop native session")
                wait(lambda:evaluate(page,"document.querySelector('[data-hls-subtitle]')?.shadowRoot?.querySelector('.translation')?.textContent === ''"),"global disable left subtitles visible")
                screenshot(popup,args.report.with_suffix('.png'))
                evaluate(page,"window.testMedia.remove()")
                wait(lambda:evaluate(page,"document.querySelectorAll('[data-hls-subtitle]').length === 0"),"removed player retained subtitle UI")
                result = {"passed":True,"browser":family,"capture":capture,"player_dom":args.player_dom,"cross_origin":args.capture == 'tab',"provider":"fixture Core, cloud accuracy not tested", "audio_frames":core.audio_frames,
                    "nonzero_samples":core.nonzero_samples,"seek_resets":core.resets,"checks":["production popup","global switch broadcast","site disable and page override","real browser audio + AudioWorklet","real Native Host PCM forwarding","subtitle render","seek reset","rate reset","time offset","provider failure and retry","reset rejection and retry","disable hides cached navigation and reenable restores audio","language change clears old cues","stop capture","removed player clears subtitle UI",*prefetch_checks]}
                args.report.parent.mkdir(parents=True,exist_ok=True); args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding="utf-8")
                return result
        except Exception:
            print(json.dumps({'fixture_sessions':core.sessions,'audio_frames':core.audio_frames},ensure_ascii=False),flush=True)
            if 'page' in locals():
                with contextlib.suppress(Exception):
                    screenshot(page,args.report.with_name(args.report.stem+'-page.png'))
                    print(json.dumps({"page":evaluate(page,"({url:location.href,ready:document.readyState,body:document.body?.innerText,players:document.querySelectorAll('video,audio').length,media:[...document.querySelectorAll('video,audio')].map(v=>({time:v.currentTime,paused:v.paused,ready:v.readyState,seekable:Array.from({length:v.seekable.length},(_,i)=>[v.seekable.start(i),v.seekable.end(i)])})),subtitleButtons:[...document.querySelectorAll('[data-hls-subtitle]')].map(e=>e.shadowRoot?.querySelector('button')?.textContent)})")},ensure_ascii=False),flush=True)
            if 'popup' in locals():
                with contextlib.suppress(Exception):
                    screenshot(popup,args.report.with_suffix('.png'))
                    print(json.dumps({"popup":evaluate(popup,"document.body.innerText")},ensure_ascii=False),flush=True)
            raise
        finally:
            if driver:
                with contextlib.suppress(Exception): driver.quit()
            if process: _stop_process_tree(process, root/'profile')
            core.shutdown(); core.server_close(); origin.shutdown(); origin.server_close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument('--extension',type=Path,required=True)
    parser.add_argument('--host',type=Path,required=True)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--browser-binary')
    parser.add_argument('--browser', choices=('chromium','firefox'), default='chromium')
    parser.add_argument('--firefox-driver')
    parser.add_argument('--capture', choices=('element','tab','prefetch'), default='element')
    parser.add_argument('--prefetch-fallback', action='store_true')
    parser.add_argument('--player-dom', choices=('document','shadow','dynamic-shadow','nested-shadow'), default='document')
    parser.add_argument('--system-temp', action='store_true')
    print(json.dumps(run(parser.parse_args()),ensure_ascii=False,indent=2))
