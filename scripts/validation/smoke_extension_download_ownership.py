"""Sandbox-only real-browser takeover, with actual Host/Core and IPC decisions.

Browser input is a WebDriver click (Firefox) or CDP mouse input (Edge).
Confirmation decisions use Core IPC; this
does not claim native confirmation-button or browser-chrome visual acceptance.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import threading
import time
import traceback

from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import WebDriverWait

from smoke_extension_browsers import _chromium_extension_id, _zip_firefox_extension
from smoke_extension_takeover import _debug_targets, _download_items, _extension_call, _registered_host
from smoke_v7_portable_app import Pipe, wait

PAYLOAD = bytes(range(256)) * 4096


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Windows Sandbox only'
    args.report.parent.mkdir(parents=True, exist_ok=True)
    # Chromium 的配置数据库不能放在 Sandbox 共享文件夹中。
    stage = Path('C:/QAWork') / (args.browser + '-ownership')
    stage.mkdir(exist_ok=True)
    report = {'passed': False, 'browser': args.browser, 'checks': [], 'decision_method': 'Core IPC',
              'browser_input': 'CDP mouse' if args.browser == 'edge' else 'WebDriver element click'}
    requests = Counter()

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            requests[self.path] += 1
            if self.path == '/':
                body = ("<title>Takeover ownership</title>"
                        "<a id='direct' href='/direct.zip'>direct.zip</a> "
                        "<a id='cancel' href='/cancel.zip'>cancel.zip</a> "
                        "<a id='opaque' href='/opaque'>opaque attachment</a> "
                        "<a id='disabled' href='/disabled.zip'>disabled.zip</a> "
                        "<a id='excluded' href='/excluded.zip'>excluded.zip</a> "
                        "<a id='offline' href='/offline.zip'>offline.zip</a> "
                        "<a id='preview' href='/preview.pdf'>preview</a> "
                        "<a id='github' href='https://github.com/ciaooo55/codex-account-switcher/releases/download/v0.13.14/Codex-Account-Switcher-Portable-0.13.14.exe'>GitHub portable EXE</a>").encode()
                mime = 'text/html'
            elif self.path == '/preview.pdf':
                body, mime = b'%PDF-1.4\n%%EOF', 'application/pdf'
            else:
                body, mime = PAYLOAD, 'application/octet-stream'
            self.send_response(200)
            self.send_header('Content-Type', mime)
            self.send_header('Content-Length', str(len(body)))
            if self.path == '/opaque':
                self.send_header('Content-Disposition', 'attachment; filename="opaque.zip"')
            self.end_headers()
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f'http://127.0.0.1:{server.server_port}/'
    driver = None

    def core(message):
        pipe = Pipe()
        try:
            return pipe.request(message)
        finally:
            pipe.close()

    def command(value):
        return core({'type': 'command', 'request_id': 1, 'command': value})

    def handoffs():
        return [json.loads(item) for item in core({'type': 'load_handoffs', 'request_id': 2})['items']]

    def record(name, actual=True):
        report['checks'].append({'name': name, 'actual': actual})
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
        print(name, flush=True)

    try:
        resources = args.runtime/'app/resources'
        if args.browser == 'edge':
            from selenium.webdriver.edge.options import Options
            from selenium.webdriver.edge.service import Service
            options = Options()
            options.binary_location = str(args.binary)
            for flag in ('--no-first-run', '--no-default-browser-check',
                         '--disable-features=DisableLoadExtensionCommandLineSwitch',
                         f'--user-data-dir={stage / "profile"}', f'--load-extension={args.extension}',
                         f'--disable-extensions-except={args.extension}', 'about:blank'):
                options.add_argument(flag)
            extension_id = _chromium_extension_id(args.extension)
            popup = f'chrome-extension://{extension_id}/popup.html'
            manifest = {'allowed_origins': [f'chrome-extension://{extension_id}/']}
        else:
            from selenium.webdriver.firefox.options import Options
            from selenium.webdriver.firefox.service import Service
            options = Options()
            options.binary_location = str(args.binary)
            options.add_argument('-headless')
            options.set_preference('browser.shell.checkDefaultBrowser', False)
            extension_id = json.loads((args.extension/'manifest.json').read_text(encoding='utf-8'))['browser_specific_settings']['gecko']['id']
            manifest = {'allowed_extensions': [extension_id]}
        manifest.update({'name': 'com.ciaooo55.hls_downloader', 'type': 'stdio',
                         'description': 'Isolated acceptance', 'path': str(resources/'HLSDownloaderNativeHost.exe')})
        manifest_path = stage/'native-host.json'
        manifest_path.write_text(json.dumps(manifest), encoding='utf-8')
        with _registered_host(manifest_path, args.browser):
            service = Service(executable_path=str(args.driver), log_output=str(args.report.parent/(args.browser+'-driver.log')),
                              **({'service_args': ['--allow-system-access']} if args.browser == 'firefox' else {}))
            driver = (webdriver.Edge if args.browser == 'edge' else webdriver.Firefox)(options=options, service=service)
            if args.browser == 'firefox':
                addon = stage/'extension.xpi'
                _zip_firefox_extension(args.extension, addon)
                driver.install_addon(str(addon), temporary=True)
                driver.set_context('chrome')
                uuids = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
                driver.set_context('content')
                popup = f'moz-extension://{uuids[extension_id]}/popup.html'
            driver.set_script_timeout(45)
            driver.set_page_load_timeout(25)
            driver.get(url)
            page = driver.current_window_handle
            driver.switch_to.new_window('tab')
            inspector = driver.current_window_handle
            driver.get(popup)
            ping = _extension_call(driver, "chrome.runtime.sendMessage({type:'ping'}).then(done)")
            assert ping.get('ok'), ping
            _extension_call(driver, "chrome.runtime.sendMessage({type:'set-takeover-settings',enabled:true,minimumBytes:0}).then(done)")
            _extension_call(driver, "chrome.storage.local.set({excludedHosts:[]}).then(()=>done(true))")
            driver.execute_script('window.createdDownloads=[];chrome.downloads.onCreated.addListener(item=>window.createdDownloads.push({id:item.id,url:item.url}));')
            report['browser_version'] = driver.capabilities.get('browserVersion')
            report['extension_background_sha256'] = hashlib.sha256((args.extension/'background.js').read_bytes()).hexdigest()

            def events():
                driver.switch_to.window(inspector)
                return driver.execute_script('return window.createdDownloads')

            def click(name):
                driver.switch_to.window(page)
                driver.get(url)
                if args.browser == 'edge':
                    # EdgeDriver 的元素查找在沙盒中可能持有过期的 JS context；
                    # 从当前 DOM 定位，再发送真实鼠标输入，仍验证 isTrusted 点击路径。
                    document = driver.execute_cdp_cmd('DOM.getDocument', {})['root']['nodeId']
                    node = driver.execute_cdp_cmd('DOM.querySelector', {'nodeId': document, 'selector': '#'+name})['nodeId']
                    assert node, name
                    quad = driver.execute_cdp_cmd('DOM.getBoxModel', {'nodeId': node})['model']['content']
                    point = {'x': sum(quad[::2])/4, 'y': sum(quad[1::2])/4, 'button':'left', 'clickCount':1}
                    driver.execute_cdp_cmd('Input.dispatchMouseEvent', {'type':'mousePressed', 'buttons':1, **point})
                    driver.execute_cdp_cmd('Input.dispatchMouseEvent', {'type':'mouseReleased', 'buttons':0, **point})
                else:
                    WebDriverWait(driver, 15).until(EC.element_to_be_clickable((By.ID, name))).click()

            def pending(before):
                return wait(lambda: next((h for h in handoffs() if h['id'] not in before and h['status'] == 'pending'), None), 'No new desktop offer', 40)

            def absent():
                return not _download_items(driver, inspector)

            # 对明确链接和服务器附件分别验证；零创建事件与事后删除是不同的验收结果。
            for name in ('direct', 'cancel', 'opaque', 'github'):
                before = {h['id'] for h in handoffs()}
                prior_events = len(events())
                click(name)
                offered = pending(before)
                wait(absent, 'Browser copy remained while confirmation pending', 15)
                delta = events()[prior_events:]
                if name != 'opaque' or args.browser == 'firefox':
                    assert not delta, delta
                if name in ('cancel', 'github'):
                    command({'kind': 'reject_handoff', 'handoff_id': offered['id'], 'suppress_site_kind': False})
                    assert absent()
                    if name == 'cancel':
                        assert requests['/cancel.zip'] == (0 if args.browser == 'edge' else 1), requests
                    record(f'{name}: cancel starts no browser fallback', {'created_events': delta})
                else:
                    command({'kind': 'accept_handoff', 'handoff_id': offered['id'], 'filename': name+'.zip', 'download_dir': ''})
                    def finished():
                        matches = [t for t in core({'type':'snapshot','request_id':3})['tasks'] if t['url'].endswith('/'+name if name == 'opaque' else '/direct.zip')]
                        if matches and matches[-1]['status'] == 'failed':
                            raise AssertionError(matches[-1])
                        return matches[-1] if matches and matches[-1]['status'] == 'completed' else None
                    task = wait(finished, 'Desktop download did not complete', 70)
                    output = Path(task['output_path'])
                    assert output.read_bytes() == PAYLOAD
                    assert absent()
                    record(f'{name}: one Core file, no browser copy', {'created_events': delta, 'sha256': hashlib.sha256(output.read_bytes()).hexdigest()})
                    command({'kind':'task_action','task_id':task['task_id'],'action':'delete_files'})

            # 明确的禁用、排除、离线都保留原浏览器功能，且只恢复一次。
            for name in ('disabled', 'excluded', 'offline'):
                driver.switch_to.window(inspector)
                _extension_call(driver, "chrome.runtime.sendMessage({type:'set-takeover-settings',enabled:arguments[0],minimumBytes:0}).then(done)", name != 'disabled')
                _extension_call(driver, 'chrome.storage.local.set(arguments[0]).then(()=>done(true))', {'excludedHosts': ['127.0.0.1'] if name == 'excluded' else []})
                if name == 'offline':
                    # 用不存在的可执行文件阻断本测试 Host；删除 HKCU 值可能退回 HKLM。
                    import winreg
                    vendor = 'Microsoft\\Edge' if args.browser == 'edge' else 'Mozilla'
                    offline_manifest = stage/'offline-host.json'
                    offline_manifest.write_text(json.dumps({**manifest, 'path': str(stage/'missing-native-host.exe')}),encoding='utf-8')
                    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, f'Software\\{vendor}\\NativeMessagingHosts\\com.ciaooo55.hls_downloader', 0, winreg.KEY_SET_VALUE) as key:
                        winreg.SetValueEx(key, '', 0, winreg.REG_SZ, str(offline_manifest))
                    import psutil
                    stopped = []
                    for process in psutil.process_iter(['exe']):
                        if process.info['exe'] and Path(process.info['exe']).resolve() == (resources/'HLSDownloaderNativeHost.exe').resolve():
                            process.terminate()
                            stopped.append(process)
                    _, alive = psutil.wait_procs(stopped, timeout=5)
                    assert not alive, 'Test NativeHost did not stop'
                    probe = _extension_call(driver, "chrome.runtime.sendMessage({type:'ping'}).then(done)")
                    report['offline_probe'] = probe
                    assert probe.get('ok') is not True, probe
                count = len(events())
                click(name)
                def browser_done():
                    matches = [d for d in _download_items(driver, inspector) if d['url'].endswith('/'+name+'.zip')]
                    return matches[0] if len(matches) == 1 and matches[0]['state'] == 'complete' else None
                item = wait(browser_done, f'{name} browser fallback failed', 55)
                assert Path(item['filename']).read_bytes() == PAYLOAD
                assert len(events()) == count + 1
                assert requests['/'+name+'.zip'] == 1, requests
                record(f'{name}: original browser transfer exactly once')
                _extension_call(driver, 'chrome.downloads.removeFile(arguments[0]).then(()=>chrome.downloads.erase({id:arguments[0]})).then(done)', item['id'])
            report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc()
        if driver:
            for name, capture in (
                ('failure_targets', lambda: _debug_targets(driver) if args.browser == 'edge' else []),
                ('failure_screenshot', lambda: driver.save_screenshot(str(args.report.with_suffix('.png')))),
                ('failure_url', lambda: driver.current_url),
                ('failure_html', lambda: driver.page_source[:12000]),
                ('failure_handoffs', lambda: [{'id':h['id'],'status':h['status'],'url':h.get('offer',{}).get('url')} for h in handoffs()]),
                ('failure_downloads', lambda: _download_items(driver, inspector)),
                ('failure_storage', lambda: _extension_call(driver, "Promise.all([chrome.storage.local.get(['enabled','minimumBytes','excludedHosts']),chrome.storage.session.get(null)]).then(done)")),
            ):
                try:
                    report[name] = capture()
                except Exception as error:
                    report[name+'_error'] = str(error)
    finally:
        if driver:
            driver.quit()
        server.shutdown()
        server.server_close()
        report['requests'] = dict(requests)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--browser', choices=('edge','firefox'), required=True)
    for name in ('runtime','binary','driver','extension','report'):
        parser.add_argument('--'+name, type=Path, required=True)
    result = run(parser.parse_args())
    print(json.dumps({'passed': result['passed'], 'error': result.get('error')}, ensure_ascii=False))
    raise SystemExit(0 if result['passed'] else 1)
