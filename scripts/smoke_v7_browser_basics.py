"""Real browser -> installed Native Host -> desktop confirmation -> media file."""
from __future__ import annotations

import argparse
import contextlib
import ctypes
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import re
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import traceback
import uuid
import zipfile
from urllib.parse import quote
from urllib.request import urlopen

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.common.action_chains import ActionChains
from selenium.webdriver.support.ui import Select
from selenium.common.exceptions import StaleElementReferenceException
from selenium.webdriver.firefox.options import Options as FirefoxOptions
from selenium.webdriver.firefox.service import Service as FirefoxService
from selenium.webdriver.edge.options import Options as EdgeOptions
from selenium.webdriver.edge.service import Service as EdgeService
from ctypes import wintypes

from smoke_extension_media import _find_firefox, _find_edge, _open_first_media_actions
from smoke_v7_browser_download_confirmation import click_download_confirmation, presenter_window
from smoke_v7_presenter_visual import user32 as native_user32
from smoke_v7_portable_app import Pipe, wait

PUBLIC = [
    ('hls', 'https://hlsjs.video-dev.org/demo/?src=https%3A%2F%2Ftest-streams.mux.dev%2Fx36xhzz%2Fx36xhzz.m3u8'),
    ('dash', 'https://reference.dashif.org/dash.js/latest/samples/getting-started/manual-load-single-video.html'),
]


def run(args):
    runtime = args.runtime.resolve()
    extension = args.extension.resolve()
    report = {'passed': False, 'browser': args.browser, 'checks': [], 'components': {}}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    for binary in ['HLSDownloaderEngine.exe', 'HLSDownloaderNativeHost.exe', 'HLSDownloaderPresenter.exe']:
        with (runtime / 'app/resources' / binary).open('rb') as stream:
            report['components'][binary] = hashlib.file_digest(stream, 'sha256').hexdigest()
    report['extension_sha256'] = {str(p.relative_to(extension)): hashlib.sha256(p.read_bytes()).hexdigest()
                                  for p in extension.rglob('*') if p.is_file()}
    created = set()
    driver = None
    selection_server = None

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
        return driver.execute_async_script('const [m,done]=arguments;browser.runtime.sendMessage(m).then(done,e=>done({ok:false,error:String(e)}))', value)

    def toolbar_state():
        # 原生弹窗在独立进程中，Marionette 不能把 XUL browser 当 HTML iframe 切换。
        source = """sendAsyncMessage('hls-qa-popup', {
            ready:content.document.documentElement.dataset.popupReady,
            text:content.document.body.textContent,
            scale:content.devicePixelRatio,
            buttons:[...content.document.querySelectorAll('button')].map(b=>{const r=b.getBoundingClientRect();return {text:b.textContent.trim(),title:b.title,pressed:b.getAttribute('aria-pressed'),disabled:b.disabled,x:content.mozInnerScreenX+r.x+r.width/2,y:content.mozInnerScreenY+r.y+r.height/2}})
        });"""
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
                options = EdgeOptions()
                options.binary_location = str(_find_edge())
                options.add_argument(f'--user-data-dir={stage / "profile"}')
                options.add_argument('--disable-features=DisableLoadExtensionCommandLineSwitch')
                options.add_argument(f'--load-extension={extension}')
                options.add_argument('--autoplay-policy=no-user-gesture-required')
                options.add_argument('--no-first-run')
                options.page_load_strategy = 'eager'
                driver = webdriver.Edge(options=options, service=EdgeService(str(args.driver)))
                popup_url = 'chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/popup.html'
            driver.set_window_size(1280, 900)
            driver.set_page_load_timeout(45)
            driver.set_script_timeout(20)
            if args.stage == 'entries':
                driver.get('https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html')
                token = uuid.uuid4().hex
                base = 'https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm'
                url = base + '?hls-entry=' + token
                hashes={url:'c6f8a348953395598a9a73b9bab1676436410797bce9f398f4be1531d6e76dda'}
                link = driver.execute_script('const a=document.createElement("a");a.href=arguments[0];a.textContent="QA download";document.body.append(a);return a', url)
                def context_command(element, label):
                    driver.execute_script('arguments[0].scrollIntoView({block:"center"})',element)
                    ActionChains(driver).context_click(element).perform()
                    driver.set_context('chrome')
                    item = wait(lambda: driver.execute_script("return [...document.querySelectorAll('menuitem')].find(e=>(e.getAttribute('label')||'')===arguments[0])||null", label), 'Context command missing: '+label, 10)
                    driver.execute_script('const menu=arguments[0].parentNode.parentNode;if(menu.localName==="menu")menu.openMenu(true)', item)
                    wait(lambda:item.is_displayed(),'Context item not visible: '+label,10)
                    report.setdefault('context_controls',[]).append(driver.execute_script('const e=arguments[0],r=e.getBoundingClientRect();return {label:e.getAttribute("label"),disabled:e.disabled,x:r.x,y:r.y,width:r.width,height:r.height}',item))
                    ActionChains(driver).click(item).perform()
                    driver.set_context('content')
                def finish(url):
                    before={t['task_id'] for t in tasks()}
                    expected={url} if isinstance(url,str) else set(url)
                    wait(lambda:presenter_window(runtime=runtime), 'Context entry has no desktop confirmation', 15)
                    click_download_confirmation()
                    def completed():
                        found=[t for t in tasks() if t['url'] in expected and t['task_id'] not in before]
                        created.update(t['task_id'] for t in found)
                        if found and found[-1]['status']=='failed':raise AssertionError(found[-1])
                        return found[-1] if found and found[-1]['status']=='completed' else None
                    task=wait(completed,'Context entry did not complete',60)
                    assert hashlib.sha256(Path(task['output_path']).read_bytes()).hexdigest()==hashes[task['url']]
                    record('context-entry-file-verified',task['url'])
                    return task['url']
                context_command(link, '使用 HLS Downloader 下载')
                finish(url);record('actual-link-context-menu-confirmation-download-sha256')
                urls=[base+'?hls-selection='+token+'-0',base.replace('flower.webm','flower.mp4')+'?hls-selection='+token+'-1']
                hashes[urls[0]]=hashes[url]
                with urlopen(urls[1],timeout=30) as response: hashes[urls[1]]=hashlib.sha256(response.read()).hexdigest()
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
                ActionChains(driver).click(shadow('.pin')).perform()
                driver.execute_script('window.__hlsQaClicks=[];document.querySelector("hls-downloader-media-panel").shadowRoot.addEventListener("click",e=>window.__hlsQaClicks.push({text:e.target.textContent,trusted:e.isTrusted}),true)')
                windows=[]
                @ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
                def locate_browser(hwnd,_):
                    owner=wintypes.DWORD()
                    native_user32.GetWindowThreadProcessId(hwnd,ctypes.byref(owner))
                    if owner.value==driver.capabilities['moz:processID'] and native_user32.IsWindowVisible(hwnd):windows.append(hwnd)
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
                    native_user32.SetForegroundWindow(windows[0])
                    before_click=driver.execute_script('return window.__hlsQaClicks.length')
                    x,y=driver.execute_script('const r=arguments[0].getBoundingClientRect();return [(mozInnerScreenX+r.x+r.width/2)*devicePixelRatio,(mozInnerScreenY+r.y+r.height/2)*devicePixelRatio]',button)
                    native_user32.SetCursorPos(round(x),round(y));native_user32.mouse_event(2,0,0,0,0);native_user32.mouse_event(4,0,0,0,0)
                    wait(lambda:driver.execute_script('return window.__hlsQaClicks.length')>before_click,'Native page click was not delivered',5)
                    report['selection_clicks']=driver.execute_script('return window.__hlsQaClicks')
                    received.append(finish(wanted))
                assert set(received)==set(urls) and len(received)==len(urls),received
                record('actual-selection-context-menu-two-links-confirmation-download-sha256')
                report['passed']=True
                return report
            if args.stage == 'errors':
                before = {t['task_id'] for t in tasks()}
                driver.get('https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html')
                link = driver.execute_script('const a=document.createElement("a");a.href="https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm";a.textContent="QA download link";a.style="position:fixed;top:10px;left:10px;z-index:2147483647;background:white;padding:20px";document.body.append(a);return a')
                ActionChains(driver).context_click(link).perform()
                driver.set_context('chrome')
                item = wait(lambda: driver.execute_script("return [...document.querySelectorAll('menuitem')].find(e=>(e.getAttribute('label')||'')==='使用 HLS Downloader 下载')||null"), 'Real extension context menu missing', 10)
                driver.execute_script('const menu=arguments[0].parentNode.parentNode;if(menu.localName==="menu")menu.openMenu(true)',item)
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
                driver.get('https://interactive-examples.mdn.mozilla.net/pages/tabbed/video.html')
                for frame in driver.find_elements(By.TAG_NAME, 'iframe'):
                    driver.switch_to.frame(frame)
                    if driver.find_elements(By.TAG_NAME, 'video'): break
                    driver.switch_to.default_content()
                driver.execute_script('const v=document.querySelector("video");if(v){v.muted=true;v.loop=true;v.play().catch(()=>{})}')
                wait(lambda: driver.execute_script('return Boolean(document.querySelector("hls-downloader-media-panel")?.shadowRoot?.querySelector(".video-download:not(.identifying)"))'), 'Public video not recognized', 25)
                driver.switch_to.default_content()
                driver.set_context('chrome')
                user32 = ctypes.WinDLL('user32')
                windows = []
                def visible_browser(hwnd, _):
                    pid = wintypes.DWORD()
                    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
                    if pid.value == driver.capabilities['moz:processID'] and user32.IsWindowVisible(hwnd): windows.append(hwnd)
                    return True
                enum_callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)(visible_browser)
                user32.EnumWindows(enum_callback, 0)
                assert windows, 'Native Firefox window missing'
                browser_hwnd = windows[0]
                user32.SetWindowPos(browser_hwnd, wintypes.HWND(-1), 0,0,0,0,0x0001|0x0002|0x0040)
                user32.SetForegroundWindow(browser_hwnd)
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
                    state = toolbar_state()
                    b = next(b for b in state['buttons'] if (b['text']==text if text else b['title']==title))
                    assert not b['disabled'], b
                    user32 = ctypes.WinDLL('user32')
                    user32.SetCursorPos(round(b['x']*state['scale']), round(b['y']*state['scale']))
                    user32.mouse_event(2,0,0,0,0);user32.mouse_event(4,0,0,0,0)
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
                click_download_confirmation(accept=False)
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
                click_download_confirmation()
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
                    driver.set_context('content')
                    report['page_error']=driver.execute_script('return document.querySelector("hls-downloader-media-panel")?.shadowRoot?.querySelector(".result")?.textContent')
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
            if args.stage=='entries' and 'token' in locals():
                markers=['hls-entry='+token,'hls-selection='+token+'-']
                created.update(t['task_id'] for t in tasks() if any(m in t['url'] for m in markers))
                for encoded in core({'type':'load_handoffs','request_id':3})['items']:
                    handoff=json.loads(encoded)
                    if handoff.get('status')=='pending' and any(m in handoff.get('offer',{}).get('url','') for m in markers):
                        core({'type':'command','request_id':4,'command':{'kind':'reject_handoff','handoff_id':handoff['id'],'suppress_site_kind':False}})
            for task_id in created:
                core({'type': 'command', 'request_id': 2, 'command': {'kind': 'task_action', 'task_id': task_id, 'action': 'delete_files'}})
            report['user_tasks_preserved'] = initial <= {t['task_id'] for t in tasks()}
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf8')
    return report


if __name__ == '__main__':
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
