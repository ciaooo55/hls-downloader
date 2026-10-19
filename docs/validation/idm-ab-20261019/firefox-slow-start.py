import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
from selenium.webdriver.common.action_chains import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys

from smoke_extension_takeover import _create_driver, _registered_host, _extension_call, _click_download, _download_items
from smoke_v7_portable_app import Pipe

ROOT = Path(r'C:\HLSValidation\idm-ab-20261019')
WORK = Path(r'C:\QAWork\firefox-slow-start')
PAYLOAD = bytes(range(256)) * 4096
RESULT = {'passed': False, 'scope': 'real Firefox + production extension + Native Host/Core; accept/reject via Core API, not rendered button input', 'checks': []}


def save():
    (ROOT / 'firefox-slow-start.json').write_text(json.dumps(RESULT, indent=2), encoding='utf-8')


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *_):
        pass

    def do_GET(self):
        RESULT.setdefault('server_events', []).append({'event': 'request', 'path': self.path, 'time': time.time() * 1000})
        if self.path == '/':
            body = (f'<html><body><a id="slow" download href="/slow.bin">download</a><a id="reject" download href="/reject.bin">reject</a><a id="reload" download href="/reload.bin">reload</a><a id="failure" download href="/failure.bin">failure</a><a id="alt" download href="/alt.bin">alt</a><a id="cross" download href="http://localhost:{self.server.server_port}/cross.bin">cross</a></body></html>').encode()
            self.send_response(200)
            self.send_header('Content-Type', 'text/html')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path == '/failure.bin' and self.headers.get('Accept-Encoding') == 'identity':
            self.send_response(403)
            self.send_header('Content-Length', '0')
            self.end_headers()
            return
        start, end = 0, len(PAYLOAD) - 1
        header = self.headers.get('Range', '')
        if header.startswith('bytes='):
            first, _, last = header[6:].partition('-')
            start, end = int(first), int(last) if last else end
        self.send_response(206 if header else 200)
        self.send_header('Content-Type', 'application/octet-stream')
        self.send_header('Content-Disposition', f'attachment; filename="{self.path[1:]}"')
        self.send_header('Accept-Ranges', 'bytes')
        self.send_header('Content-Length', str(end - start + 1))
        if header:
            self.send_header('Content-Range', f'bytes {start}-{end}/{len(PAYLOAD)}')
        self.end_headers()
        self.wfile.flush()
        RESULT['server_events'].append({'event': 'headers-sent', 'path': self.path, 'time': time.time() * 1000})
        if self.headers.get('Accept-Encoding') != 'identity':
            time.sleep(8)
            RESULT['server_events'].append({'event': 'browser-body-ready', 'path': self.path, 'time': time.time() * 1000})
        with contextlib.suppress(BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            self.wfile.write(PAYLOAD[start:end + 1])


def core(message):
    p = Pipe()
    try:
        return p.request(message)
    finally:
        p.close()


def main():
    WORK.mkdir(parents=True, exist_ok=True)
    RESULT['background_sha256'] = hashlib.sha256(Path(r'C:\HLSExtension\firefox-mv3\background.js').read_bytes()).hexdigest()
    downloads = WORK / 'downloads'
    downloads.mkdir(exist_ok=True)
    manifest = WORK / 'host.json'
    manifest.write_text(json.dumps({'name': 'com.ciaooo55.hls_downloader', 'description': 'isolated validation', 'path': r'C:\HLSRuntime\app\resources\HLSDownloaderNativeHost.exe', 'type': 'stdio', 'allowed_extensions': ['hls-downloader@ciaooo55']}), encoding='utf-8')
    addon_manifest = json.loads(Path(r'C:\HLSExtension\firefox-mv3\manifest.json').read_text(encoding='utf-8'))
    manifest_data = json.loads(manifest.read_text())
    manifest_data['allowed_extensions'] = [addon_manifest['browser_specific_settings']['gecko']['id']]
    manifest.write_text(json.dumps(manifest_data), encoding='utf-8')
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    driver = None
    try:
        with _registered_host(manifest, 'firefox'):
            driver = _create_driver('firefox', Path(r'C:\Firefox\firefox.exe'), Path(r'C:\HLSExtension\firefox-mv3'), WORK / 'profile', downloads, Path(r'C:\HLSValidation\idm-ab-20261019\geckodriver\geckodriver.exe'), headed=True)
            driver.get(f'http://127.0.0.1:{server.server_port}/')
            page = driver.current_window_handle
            driver.set_context('chrome')
            uuids = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
            driver.set_context('content')
            driver.switch_to.new_window('tab')
            inspector = driver.current_window_handle
            driver.get(f"moz-extension://{uuids[manifest_data['allowed_extensions'][0]]}/popup.html")
            driver.set_script_timeout(35)
            ping = _extension_call(driver, 'chrome.runtime.sendMessage({type:"ping"}).then(done,e=>done({ok:false,error:String(e)}))')
            RESULT['ping'] = ping
            assert ping.get('ok'), ping
            _extension_call(driver, 'chrome.storage.local.set({minimumBytes:0,enabled:true,excludedHosts:[]}).then(()=>done(true))')
            _extension_call(driver, '''const events=[];
chrome.webRequest.onHeadersReceived.addListener(d=>{events.push({event:'headers',type:d.type,url:d.url,time:Date.now(),requestId:d.requestId,tabId:d.tabId});chrome.storage.session.set({probeEvents:events})},{urls:['http://127.0.0.1/*']});
chrome.downloads.onCreated.addListener(d=>{events.push({event:'created',url:d.url,time:Date.now(),bytesReceived:d.bytesReceived,referrer:d.referrer,tabId:d.tabId});chrome.storage.session.set({probeEvents:events})});done(true)''')
            core({'type': 'store_setting', 'request_id': 1, 'key': 'browser_takeover_minimum_bytes', 'value': '0'})
            case_names = [] if '--alt-only' in sys.argv else ['slow', 'reject', 'reload', 'failure', 'cross']
            if '--ui-decisions' in sys.argv:
                case_names = ['slow', 'reject']
                RESULT['scope'] = 'headed Firefox + production extension + Native Host/Core; actual confirmation buttons operated through computer-use'
            for name in case_names:
                before = {json.loads(x)['id'] for x in core({'type': 'load_handoffs', 'request_id': 2})['items']}
                began_wall = time.time() * 1000
                began = time.perf_counter()
                found = []
                def observe():
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline:
                        rows = [json.loads(x) for x in core({'type': 'load_handoffs', 'request_id': 3})['items']]
                        handoff = next((r for r in rows if r['id'] not in before), None)
                        if handoff:
                            found.append((handoff, time.perf_counter()))
                            return
                        time.sleep(.05)
                observer = threading.Thread(target=observe, daemon=True)
                observer.start()
                driver.switch_to.window(page)
                ActionChains(driver).move_to_element(driver.find_element(By.ID, name)).click().perform()
                click_finished = time.perf_counter()
                observer.join(20)
                assert found, 'No desktop handoff'
                handoff, observed = found[0]
                elapsed = (observed - began) * 1000
                RESULT['checks'].append({'name': name + '-handoff-latency', 'milliseconds': elapsed, 'click_command_ms': (click_finished - began) * 1000, 'handoff': handoff['id'], 'began_wall': began_wall})
                driver.switch_to.window(inspector)
                RESULT['browser_events'] = _extension_call(driver, "chrome.storage.session.get('probeEvents').then(done)")
                save()
                if name == 'reload':
                    driver.switch_to.window(inspector)
                    with contextlib.suppress(Exception):
                        _extension_call(driver, 'done(true);chrome.runtime.reload()')
                    time.sleep(1)
                    driver.switch_to.window(page)
                    driver.switch_to.new_window('tab')
                    inspector = driver.current_window_handle
                    driver.get(f"moz-extension://{uuids[manifest_data['allowed_extensions'][0]]}/popup.html")
                    RESULT['checks'].append({'name': 'extension-reloaded-while-decision-pending'})
                    save()
                if name == 'cross':
                    assert elapsed >= 7000, 'Cross-origin download attribute must not pre-offer a navigation'
                action = 'reject_handoff' if name in ['reject', 'reload', 'cross'] else 'accept_handoff'
                command = {'kind': action, 'handoff_id': handoff['id'], 'suppress_site_kind': False}
                if name in ['slow', 'failure']:
                    command.update(filename=name + '.bin', download_dir='', trusted_ui=True)
                if '--ui-decisions' in sys.argv:
                    (ROOT / 'ui-decision.json').write_text(json.dumps({'name': name, 'handoff': handoff['id'], 'time': time.time()}))
                    deadline = time.monotonic() + 90
                    while time.monotonic() < deadline:
                        rows = [json.loads(x) for x in core({'type': 'load_handoffs', 'request_id': 4})['items']]
                        response = next(h for h in rows if h['id'] == handoff['id'])
                        if response['status'] != 'pending':
                            break
                        time.sleep(.1)
                    assert response['status'] in (['accepted'] if name == 'slow' else ['rejected', 'canceled']), response
                else:
                    response = core({'type': 'command', 'request_id': 4, 'command': command})
                assert response.get('type') != 'error', response
                RESULT['checks'].append({'name': name + '-decision', 'response': response})
                save()
                deadline = time.monotonic() + 40
                if name in ['slow', 'failure']:
                    while time.monotonic() < deadline:
                        rows = core({'type': 'snapshot', 'request_id': 5})['tasks']
                        task = next((t for t in rows if t['url'].endswith('/' + name + '.bin')), None)
                        if task and task['status'] in ['completed', 'failed']:
                            break
                        time.sleep(.1)
                    assert task and task['status'] == ('completed' if name == 'slow' else 'failed'), task
                    if name == 'slow':
                        assert hashlib.sha256(Path(task['output_path']).read_bytes()).digest() == hashlib.sha256(PAYLOAD).digest()
                while time.monotonic() < deadline:
                    driver.switch_to.window(inspector)
                    items = _download_items(driver, inspector)
                    item = next((x for x in items if x.get('url', '').endswith('/' + name + '.bin')), None)
                    body_released = any(e['event'] == 'browser-body-ready' and e['path'] == '/slow.bin' for e in RESULT.get('server_events', []))
                    if name == 'slow' and item is None and body_released and time.perf_counter() - began > 9 or name != 'slow' and item and item['state'] == 'complete':
                        break
                    time.sleep(.2)
                assert (item is None if name == 'slow' else item and item['state'] == 'complete'), item
                if item:
                    assert hashlib.sha256(Path(item['filename']).read_bytes()).digest() == hashlib.sha256(PAYLOAD).digest()
                RESULT['checks'].append({'name': name + '-browser-fallback-or-cleanup', 'item': item})
                save()
            handoffs = [json.loads(x) for x in core({'type': 'load_handoffs', 'request_id': 8})['items']]
            for name in case_names:
                matches = [h for h in handoffs if h.get('offer', {}).get('url', '').endswith('/' + name + '.bin')]
                assert len(matches) == 1, (name, matches)
            RESULT['checks'].append({'name': 'one-handoff-per-request-including-extension-reload'})
            save()
            if '--ui-decisions' in sys.argv:
                RESULT['passed'] = True
                return
            before = {h['id'] for h in handoffs}
            driver.set_context('chrome')
            driver.execute_script("Services.prefs.setBoolPref('browser.altClickSave', true)")
            RESULT['alt_click_save_enabled_in_isolated_profile'] = True
            driver.set_context('content')
            driver.switch_to.window(page)
            ActionChains(driver).key_down(Keys.ALT).move_to_element(driver.find_element(By.ID, 'alt')).click().key_up(Keys.ALT).perform()
            (ROOT / 'alt-clicked.ready').write_text('ready')
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                driver.switch_to.window(inspector)
                item = next((x for x in _download_items(driver, inspector) if x.get('url', '').endswith('/alt.bin')), None)
                assert {json.loads(x)['id'] for x in core({'type': 'load_handoffs', 'request_id': 7})['items']} == before, 'Alt bypass offered a desktop handoff'
                if item and item['state'] == 'complete':
                    assert hashlib.sha256(Path(item['filename']).read_bytes()).digest() == hashlib.sha256(PAYLOAD).digest()
                    break
                time.sleep(.2)
            else:
                raise AssertionError('Alt bypass did not finish in browser')
            RESULT['checks'].append({'name': 'alt-bypass-browser-file-sha256'})
            RESULT['responsive'] = all(c.get('milliseconds', 0) < 2000 for c in RESULT['checks'] if c.get('name') != 'cross-handoff-latency')
            RESULT['passed'] = RESULT['responsive']
    except Exception as error:
        import traceback
        RESULT['error'] = repr(error)
        RESULT['traceback'] = traceback.format_exc()
    finally:
        if driver:
            driver.quit()
        server.shutdown()
        server.server_close()
        save()


if __name__ == '__main__':
    main()
