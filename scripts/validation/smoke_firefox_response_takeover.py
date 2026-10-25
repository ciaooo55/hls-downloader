"""Verify Firefox response blocking and Chromium download-item cleanup.

Run only in a disposable Windows Sandbox. Decisions use the real Core protocol;
this is not a test of clicking the desktop confirmation buttons.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import ssl
import tempfile
import threading
import time
import traceback
from urllib.request import urlopen

import certifi
import psutil
from selenium.webdriver.common.action_chains import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.common.keys import Keys

from smoke_extension_takeover import EXTENSION_ID, IMAGE_PAYLOAD, JSON_PAYLOAD, _create_driver, _extension_call, _registered_host
from smoke_v7_portable_app import Pipe, wait

PUBLIC_URL = ('https://github.com/ciaooo55/codex-account-switcher/releases/download/'
              'v0.13.14/Codex-Account-Switcher-Portable-0.13.14.exe')
PAYLOAD = bytes(range(256)) * 8192


def run(args):
    report = {'passed': False, 'checks': [], 'browser': args.browser, 'decision_transport': 'Core protocol',
              'background_sha256': hashlib.sha256((args.extension / 'background.js').read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    driver = None
    server = None
    created = set()
    offered = set()
    offline_host_backup = None

    def record(name, actual):
        report['checks'].append({'name': name, 'actual': actual})
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
        print(name, flush=True)

    def core(value):
        pipe = Pipe()
        try:
            return pipe.request(value)
        finally:
            pipe.close()

    def tasks():
        return core({'type': 'snapshot', 'request_id': 1})['tasks']

    def handoffs():
        return [json.loads(row) for row in core({'type': 'load_handoffs', 'request_id': 1})['items']]

    def command(value):
        result = core({'type': 'command', 'request_id': 1, 'command': value})
        assert result.get('type') != 'error', result
        return result

    with tempfile.TemporaryDirectory(prefix='firefox-response-') as temporary:
        root = Path(temporary)
        downloads = root / 'browser-downloads'
        downloads.mkdir()
        manifest = root / 'host.json'
        manifest.write_text(json.dumps({
            'name': 'com.ciaooo55.hls_downloader', 'description': 'Isolated real host',
            'path': str(args.runtime / 'app/resources/HLSDownloaderNativeHost.exe'),
            'type': 'stdio',
            **({'allowed_extensions': ['hls-downloader-store@ciaooo55.com']} if args.browser == 'firefox'
               else {'allowed_origins': [f'chrome-extension://{EXTENSION_ID}/']}),
        }), encoding='utf-8')

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                if self.path == '/page':
                    body = ('<!doctype html><title>Firefox response takeover</title>'
                            + ''.join(f'<a id="{name}" href="/{name}.bin"{attribute}>{name}</a><br>'
                                      for name, attribute in [('plain', ''), ('attribute', ' download'),
                                                              ('cancel', ''), ('disabled', ''),
                                                              ('excluded', ''), ('alt', ''),
                                                              ('failed', ''), ('offline', ''),
                                                              ('inline', ' download'), ('stream', ''),
                                                              ('image', ''), ('export', '')])
                            + '<a id="redirect" href="/redirect">redirect</a><br>'
                            + '<a id="opaque" href="/opaque">opaque</a><br>'
                            + f'<a id="public" href="{PUBLIC_URL}">GitHub EXE (never executed)</a>').encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'text/html; charset=utf-8')
                elif self.path == '/redirect':
                    self.send_response(302)
                    self.send_header('Location', '/redirected.bin')
                    self.end_headers()
                    return
                elif self.path == '/failed.bin' and self.headers.get('Accept-Encoding') == 'identity':
                    self.send_error(403)
                    return
                else:
                    body = IMAGE_PAYLOAD if self.path == '/image.bin' else JSON_PAYLOAD if self.path == '/export.bin' else PAYLOAD
                    self.send_response(200)
                    mime = {'/inline.bin': 'application/pdf', '/image.bin': 'image/png', '/export.bin': 'application/json'}
                    self.send_header('Content-Type', mime.get(self.path, 'application/octet-stream'))
                    if self.path != '/opaque':
                        mode = 'inline' if self.path == '/inline.bin' else 'attachment'
                        self.send_header('Content-Disposition', f'{mode}; filename="{self.path[1:]}"')
                if self.path != '/stream.bin':
                    self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                with contextlib.suppress(ConnectionError):
                    self.wfile.write(body)

        try:
            server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            page_url = f'http://127.0.0.1:{server.server_port}/page'
            with _registered_host(manifest, args.browser):
                driver = _create_driver(args.browser, args.binary, args.extension, root / 'profile',
                                        downloads, args.driver, headed=args.headed)
                report['browser_version'] = driver.capabilities.get('browserVersion')
                driver.set_script_timeout(70)
                driver.set_page_load_timeout(30)
                driver.get(page_url)
                page = driver.current_window_handle
                if args.browser == 'firefox':
                    driver.set_context('chrome')
                    uuids = json.loads(driver.execute_script(
                        "return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
                    driver.set_context('content')
                    popup_url = f'moz-extension://{uuids["hls-downloader-store@ciaooo55.com"]}/popup.html'
                else:
                    popup_url = f'chrome-extension://{EXTENSION_ID}/popup.html'
                driver.switch_to.new_window('tab')
                inspector = driver.current_window_handle
                driver.get(popup_url)
                # The event log survives erase(), so cancelling/erasing a browser item cannot pass.
                driver.execute_script('window.downloadEvents=[]; chrome.downloads.onCreated.addListener('
                                      'item=>window.downloadEvents.push({id:item.id,url:item.url}));')

                def extension(script, *values):
                    driver.switch_to.window(inspector)
                    return _extension_call(driver, script, *values)

                def message(value):
                    return extension('chrome.runtime.sendMessage(arguments[0]).then(done,e=>done({error:String(e)}))', value)

                assert message({'type': 'ping'}).get('ok') is True
                record('real-host-connected', True)
                core({'type': 'store_settings', 'request_id': 1, 'values': {'legal_terms_accepted': True}})
                original = {task['task_id'] for task in tasks()}

                def browser_state():
                    return extension('Promise.all([chrome.downloads.search({}),Promise.resolve(window.downloadEvents)])'
                                     '.then(([items,events])=>done({items,events}))')

                def assert_no_browser_download(name):
                    def cleared():
                        value = browser_state()
                        return value if not value['items'] else None
                    state = wait(cleared, name + ': browser copy not removed', 15)
                    wait(lambda: not list(downloads.iterdir()), name + ': browser file not removed', 15)
                    panel = None
                    if args.browser == 'firefox':
                        assert not state['events'], state
                        driver.set_context('chrome')
                        panel = driver.execute_script("return {state:window.DownloadsPanel?.panel?.state || 'closed'}")
                        driver.set_context('content')
                        assert panel['state'] == 'closed', panel
                    record(name, {'items': state['items'], 'created_events': state['events'], 'panel': panel})

                def click(name, alt=False):
                    driver.switch_to.window(page)
                    driver.get(page_url)
                    element = driver.find_element(By.ID, name)
                    if alt:
                        ActionChains(driver).key_down(Keys.ALT).click(element).key_up(Keys.ALT).perform()
                    else:
                        element.click()

                def accepted_case(name, suffix, reject=False, expect_failed=False):
                    old_handoffs = {row['id'] for row in handoffs()}
                    before = {task['task_id'] for task in tasks()}
                    click(name)
                    def pending():
                        found = [row for row in handoffs() if row['id'] not in old_handoffs]
                        assert len(found) <= 1, found
                        return found[0] if found else None
                    handoff = wait(pending, f'{name}: desktop confirmation not queued', 35)
                    offered.add(handoff['id'])
                    assert handoff['status'] == 'pending', handoff
                    assert before == {task['task_id'] for task in tasks()}
                    assert_no_browser_download(name + '-before-decision')
                    command({'kind': 'reject_handoff' if reject else 'accept_handoff',
                             'handoff_id': handoff['id'], 'filename': suffix,
                             'download_dir': '', 'trusted_ui': True,
                             'suppress_site_kind': False})
                    if reject:
                        time.sleep(1)
                        assert before == {task['task_id'] for task in tasks()}
                    else:
                        def completed():
                            found = [task for task in tasks() if task['task_id'] not in before]
                            created.update(task['task_id'] for task in found)
                            assert len(found) <= 1, found
                            if found and found[0]['status'] == 'failed' and not expect_failed:
                                raise AssertionError(found[0])
                            return found[0] if found and found[0]['status'] in ['completed', 'failed'] else None
                        task = wait(completed, f'{name}: Core did not complete', 180)
                        if expect_failed:
                            assert task['status'] == 'failed' and task.get('error_message'), task
                            record(name + '-Core-visible-failure', task['error_message'])
                            assert_no_browser_download(name + '-after-failure')
                            return
                        output = Path(task['output_path'])
                        # Public EXE is treated as data, never started.
                        if name == 'public':
                            with urlopen('https://api.github.com/repos/ciaooo55/codex-account-switcher/releases/tags/v0.13.14',
                                         timeout=60, context=ssl.create_default_context(cafile=certifi.where())) as response:
                                release = json.load(response)
                            asset = next(asset for asset in release['assets'] if asset['name'] == suffix)
                            assert asset['digest'].startswith('sha256:'), asset
                            assert output.stat().st_size == asset['size'], asset['size']
                            expected_hash = asset['digest'].removeprefix('sha256:')
                        else:
                            expected = IMAGE_PAYLOAD if name == 'image' else JSON_PAYLOAD if name == 'export' else PAYLOAD
                            expected_hash = hashlib.sha256(expected).hexdigest()
                        assert hashlib.sha256(output.read_bytes()).hexdigest() == expected_hash
                        record(name + '-Core-output', {'bytes': output.stat().st_size, 'sha256': expected_hash})
                    assert_no_browser_download(name + '-after-decision')

                for name, suffix in [('plain', 'plain.bin'), ('attribute', 'attribute.bin'),
                                     ('redirect', 'redirected.bin'), ('inline', 'inline.bin'),
                                     ('opaque', 'opaque.bin'), ('stream', 'stream.bin'),
                                     ('image', 'image.png'), ('export', 'export.json')]:
                    accepted_case(name, suffix)
                before = {row['id'] for row in handoffs()}
                driver.switch_to.window(page)
                driver.get(page_url)
                received = driver.execute_async_script('const done=arguments[0]; fetch("/xhr.bin").then(r=>r.arrayBuffer()).then(b=>done(b.byteLength))')
                assert received == len(PAYLOAD), received
                assert before == {row['id'] for row in handoffs()}
                assert_no_browser_download('XHR-response-left-to-page')
                accepted_case('cancel', 'cancel.bin', reject=True)
                accepted_case('failed', 'failed.bin', expect_failed=True)
                if args.public:
                    accepted_case('public', Path(PUBLIC_URL).name)

                # Browser fallback is intentional only when explicitly bypassed/disabled.
                for name in ['disabled', 'excluded', 'offline', 'alt']:
                    if name == 'disabled':
                        assert message({'type': 'set-takeover-settings', 'enabled': False, 'minimumBytes': 0}).get('ok')
                    if name == 'excluded':
                        extension("chrome.storage.local.set({excludedHosts:['127.0.0.1']}).then(()=>done(true))")
                    if name == 'offline':
                        registration = json.loads(manifest.read_text(encoding='utf-8'))
                        registration['path'] = str(root / 'missing-host.exe')
                        manifest.write_text(json.dumps(registration), encoding='utf-8')
                        stopped = []
                        for process in psutil.process_iter(['name', 'exe']):
                            if process.info['name'] != 'HLSDownloaderNativeHost.exe' or not process.info['exe']:
                                continue
                            if Path(process.info['exe']).resolve() != (args.runtime / 'app/resources/HLSDownloaderNativeHost.exe').resolve():
                                continue
                            # 本脚本仅在独立沙盒运行；按本轮 runtime 的精确路径关闭所有连接，
                            # 不能假定浏览器派生的每个 Host 都仍在 WebDriver 的父进程链上。
                            process.terminate()
                            process.wait(5)
                            stopped.append(process.pid)
                        assert stopped, 'No isolated Firefox Native Host to disconnect'
                        host_path = args.runtime / 'app/resources/HLSDownloaderNativeHost.exe'
                        offline_host_backup = host_path.with_suffix('.offline')
                        host_path.rename(offline_host_backup)
                        assert not message({'type': 'ping'}).get('ok'), 'Offline fault did not make Native Host unavailable'
                        record('offline-fault-confirmed', {'stopped': stopped, 'host_executable_absent': not host_path.exists()})
                    previous = {row['id'] for row in handoffs()}
                    click(name, alt=name == 'alt')
                    if name == 'alt':
                        # Firefox 的 Alt 点击由浏览器偏好决定是否打开另存为；这里只核对不产生桌面接管。
                        time.sleep(1)
                        assert previous == {row['id'] for row in handoffs()}
                        record('alt-click-does-not-create-desktop-offer', browser_state())
                        continue
                    def browser_completed():
                        found = [item for item in browser_state()['items'] if item['url'].endswith('/' + name + '.bin')]
                        return found[0] if found and found[0]['state'] == 'complete' else None
                    item = wait(browser_completed, name + ': intentional browser download did not complete', 35)
                    assert hashlib.sha256(Path(item['filename']).read_bytes()).digest() == hashlib.sha256(PAYLOAD).digest()
                    assert previous == {row['id'] for row in handoffs()}
                    record(name + '-browser-bypass-no-desktop-offer', True)
                    extension('chrome.downloads.removeFile(arguments[0]).then(()=>chrome.downloads.erase({id:arguments[0]})).then(done)', item['id'])
                    extension('window.downloadEvents=[]; done(true)')
                    if name == 'disabled':
                        assert message({'type': 'set-takeover-settings', 'enabled': True, 'minimumBytes': 0}).get('ok')
                    if name == 'excluded':
                        extension('chrome.storage.local.set({excludedHosts:[]}).then(()=>done(true))')
                    if name == 'offline':
                        offline_host_backup.rename(args.runtime / 'app/resources/HLSDownloaderNativeHost.exe')
                        offline_host_backup = None
                        registration['path'] = str(args.runtime / 'app/resources/HLSDownloaderNativeHost.exe')
                        manifest.write_text(json.dumps(registration), encoding='utf-8')
                        assert message({'type': 'ping'}).get('ok'), 'Native Host did not reconnect'
                report['original_tasks_preserved'] = original <= {task['task_id'] for task in tasks()}
                report['passed'] = True
        except Exception as error:
            report['error'] = str(error)
            report['traceback'] = traceback.format_exc()
            if driver:
                with contextlib.suppress(Exception):
                    report['browser_state'] = browser_state()
                with contextlib.suppress(Exception):
                    report['handoffs'] = handoffs()
        finally:
            if offline_host_backup and offline_host_backup.exists():
                offline_host_backup.rename(args.runtime / 'app/resources/HLSDownloaderNativeHost.exe')
            if driver:
                with contextlib.suppress(Exception):
                    driver.quit()
            with contextlib.suppress(Exception):
                for handoff in handoffs():
                    if handoff['id'] in offered and handoff['status'] == 'pending':
                        command({'kind': 'reject_handoff', 'handoff_id': handoff['id'], 'suppress_site_kind': False})
            for task_id in created:
                with contextlib.suppress(Exception):
                    command({'kind': 'task_action', 'task_id': task_id, 'action': 'delete_files'})
            if server:
                server.shutdown()
                server.server_close()
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ('runtime', 'extension', 'binary', 'driver', 'report'):
        parser.add_argument('--' + flag, type=Path, required=True)
    parser.add_argument('--headed', action='store_true')
    parser.add_argument('--public', action='store_true')
    parser.add_argument('--browser', choices=('firefox', 'edge', 'chrome'), default='firefox')
    arguments = parser.parse_args()
    result = run(arguments)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    raise SystemExit(0 if result['passed'] else 1)
