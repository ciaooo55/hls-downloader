"""Verify response ownership and completed-file import in real browsers.

An existing runtime is for disposable Windows Sandbox runs. The isolated
component mode starts its own Core/data/pipe and restores host registration.
Decisions use the real Core protocol; desktop confirmation buttons are not tested.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
from validation_workspace import test_tmp
import threading
import time
import traceback
import uuid
from urllib.parse import urlsplit
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
POST_BODY = b'{"token":"signed%2Fvalue+with=bytes","sequence":7}'
AUTHORIZATION = 'Bearer browser-validation-only'


@contextlib.contextmanager
def isolated_runtime(args):
    """Run current components without opening the user's tasks or Core pipe."""
    workspace = Path(__file__).resolve().parents[2]
    if not args.report.resolve().is_relative_to(workspace):
        raise ValueError('Isolated report must stay in the project workspace')
    sources = {
        'HLSDownloaderEngine.exe': args.isolated_engine.resolve(),
        'HLSDownloaderNativeHost.exe': args.isolated_host.resolve(),
        'HLSDownloaderPresenter.exe': args.isolated_presenter.resolve(),
    }
    if any(not path.is_file() for path in sources.values()):
        raise ValueError('An isolated component is missing')
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='hls-native-browser-', dir=test_tmp()) as temporary:
        root = Path(temporary).resolve()
        resources = root / 'app/resources'
        resources.mkdir(parents=True)
        for name, source in sources.items():
            os.link(source, resources / name)
        pipe_name = rf'\\.\pipe\HLSDownloader.browser-validation.{uuid.uuid4().hex}'
        changes = {
            'HLS_V7_DATA_DIR': str(root / 'data'),
            'HLS_V7_DOWNLOAD_DIR': str(root / 'downloads'),
            'HLS_V7_PIPE': pipe_name,
            'HLS_V7_CORE_TCP': None,
            'HLS_V7_CORE_BIND': None,
            'HLS_V7_PRESENTER_TRACE_FILE': str(args.report.with_suffix('.presenter.log')),
        }
        previous = {name: os.environ.get(name) for name in changes}
        engine = None
        try:
            for name, value in changes.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value
            with args.report.with_suffix('.core.log').open('w', encoding='utf-8') as log:
                engine = subprocess.Popen([str(resources / 'HLSDownloaderEngine.exe')], cwd=resources,
                                          stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)

                def ready():
                    if engine.poll() is not None:
                        raise RuntimeError(f'Isolated Core exited with {engine.returncode}')
                    try:
                        pipe = Pipe(pipe_name)
                        pipe.close()
                        return True
                    except OSError:
                        return False

                wait(ready, 'Isolated Core did not open its pipe', 30)
                listeners = [row.laddr.port for row in psutil.Process(engine.pid).net_connections(kind='tcp')
                             if row.status == psutil.CONN_LISTEN]
                assert not listeners, f'Pipe-only Core unexpectedly opened TCP listeners: {listeners}'
                args.runtime = root
                yield {
                    'data_isolated': True, 'pipe': pipe_name,
                    'tcp_listeners': listeners,
                    'binaries': {name: {'source': str(source), 'sha256': hashlib.sha256(source.read_bytes()).hexdigest()}
                                 for name, source in sources.items()},
                }
        finally:
            # Only this fixture's executable paths qualify, including UI/Host
            # processes detached from their original browser parent.
            owned = []
            for process in psutil.process_iter(['exe']):
                with contextlib.suppress(psutil.Error, OSError):
                    if process.info['exe'] and Path(process.info['exe']).resolve().is_relative_to(root):
                        process.terminate()
                        owned.append(process)
            _, alive = psutil.wait_procs(owned, timeout=5)
            for process in alive:
                with contextlib.suppress(psutil.Error):
                    process.kill()
            psutil.wait_procs(alive, timeout=5)
            if engine is not None:
                engine.wait(timeout=5)
            for name, value in previous.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value


def run(args):
    report = {'passed': False, 'checks': [], 'browser': args.browser, 'decision_transport': 'Core protocol',
              'background_sha256': hashlib.sha256((args.extension / 'background.js').read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    driver = None
    server = None
    created = set()
    offered = set()
    offline_host_backup = None
    requests = []

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

    with tempfile.TemporaryDirectory(prefix='firefox-response-', dir=test_tmp()) as temporary:
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
                return self.reply()

            def do_POST(self):
                return self.reply(self.rfile.read(int(self.headers.get('Content-Length', '0'))))

            def reply(self, request_body=b''):
                path = urlsplit(self.path).path
                requests.append({'path': self.path, 'method': self.command,
                                 'authorization_matches': self.headers.get('Authorization') == AUTHORIZATION,
                                 'cookie_matches': 'hls-validation=session-only' in self.headers.get('Cookie', ''),
                                 'referer': self.headers.get('Referer'), 'origin': self.headers.get('Origin'),
                                 'user_agent': self.headers.get('User-Agent'),
                                 'body_sha256': hashlib.sha256(request_body).hexdigest()})
                if path.startswith('/credential-'):
                    expected_body = POST_BODY if path == '/credential-post.bin' else b''
                    if (self.path != path + '?signature=a%2Fb%2B%3D&n=1'
                            or self.headers.get('Authorization') != AUTHORIZATION
                            or 'hls-validation=session-only' not in self.headers.get('Cookie', '')
                            or request_body != expected_body):
                        self.send_error(403)
                        return
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
                            + '<form action="/form.bin" method="post"><input name="token" value="one-use">'
                            + '<button id="form" type="submit">POST download</button></form>'
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
                elif self.path == '/form.bin' and sum(row['path'] == '/form.bin' for row in requests) > 1:
                    self.send_error(410)
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
                # 保留原始创建事件，不能把事后清除记录算成从未创建。
                driver.execute_script('window.downloadEvents=[]; chrome.downloads.onCreated.addListener('
                                      'item=>window.downloadEvents.push({id:item.id,url:item.url}));')

                def extension(script, *values):
                    driver.switch_to.window(inspector)
                    return _extension_call(driver, script, *values)

                def message(value):
                    return extension('chrome.runtime.sendMessage(arguments[0]).then(done,e=>done({error:String(e)}))', value)

                ping = message({'type': 'ping'})
                assert ping.get('ok') is True
                record('real-host-connected', {'enabled': ping.get('takeover_enabled'),
                                               'minimum_bytes': ping.get('takeover_minimum_bytes')})
                core({'type': 'store_settings', 'request_id': 1, 'values': {
                    'legal_terms_accepted': True, 'download_progress_window_enabled': False,
                    'download_complete_popup_enabled': False}})
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
                        driver.set_context('chrome')
                        panel = driver.execute_script("return {state:window.DownloadsPanel?.panel?.state || 'closed'}")
                        driver.set_context('content')
                    record(name, {'items': state['items'], 'created_events': state['events'], 'panel': panel})

                def click(name, alt=False):
                    driver.switch_to.window(page)
                    driver.get(page_url)
                    element = driver.find_element(By.ID, name)
                    if alt:
                        ActionChains(driver).key_down(Keys.ALT).click(element).key_up(Keys.ALT).perform()
                    else:
                        element.click()

                def restart_host():
                    assert message({'type': 'ping'}).get('ok'), 'Native Host unavailable before restart'
                    stopped = []
                    host = (args.runtime / 'app/resources/HLSDownloaderNativeHost.exe').resolve()
                    for process in psutil.process_iter(['exe']):
                        if process.info['exe'] and Path(process.info['exe']).resolve() == host:
                            process.terminate()
                            process.wait(5)
                            stopped.append(process.pid)
                    assert stopped, 'No fixture Native Host to restart'
                    assert message({'type': 'ping'}).get('ok'), 'Native Host did not reconnect'
                    return stopped

                def reload_extension():
                    nonlocal inspector
                    previous_events = browser_state()['events']
                    background_origin = extension(
                        'chrome.runtime.getBackgroundPage().then(bg=>done(bg.performance.timeOrigin))')
                    assert isinstance(background_origin, (int, float)) and background_origin > 0, background_origin
                    diagnostics = {'before': background_origin}
                    try:
                        driver.execute_script('chrome.runtime.reload()')
                    except Exception as error:
                        diagnostics['reload_navigation'] = str(error)

                    def reloaded():
                        nonlocal inspector
                        try:
                            if inspector not in driver.window_handles:
                                # Firefox closes extension tabs during reload. Marionette
                                # cannot create a tab from that discarded context.
                                driver.switch_to.window(page)
                                driver.switch_to.new_window('tab')
                                inspector = driver.current_window_handle
                            driver.switch_to.window(inspector)
                            driver.get(popup_url)
                            current = extension(
                                'chrome.runtime.getBackgroundPage().then(bg=>done(bg.performance.timeOrigin))')
                            diagnostics['last_origin'] = current
                            return current if current != background_origin else None
                        except Exception as error:
                            # Reload invalidates the old extension document/context.
                            diagnostics['last_error'] = str(error)
                            return None

                    try:
                        current = wait(reloaded, 'Extension background did not restart', 30)
                    finally:
                        record('extension-reload-diagnostics', diagnostics)
                    extension('window.downloadEvents=arguments[0]; chrome.downloads.onCreated.addListener('
                              'item=>window.downloadEvents.push({id:item.id,url:item.url})); done(true)', previous_events)
                    assert message({'type': 'ping'}).get('ok'), 'Reloaded extension did not reconnect'
                    return {'before': background_origin, 'after': current}

                def restart_core():
                    executable = (args.runtime / 'app/resources/HLSDownloaderEngine.exe').resolve()
                    processes = [process for process in psutil.process_iter(['exe'])
                                 if process.info['exe'] and Path(process.info['exe']).resolve() == executable]
                    assert len(processes) == 1, 'Expected one isolated Core before restart'
                    previous = processes[0].pid
                    processes[0].kill()
                    processes[0].wait(5)
                    # The real Native Host must start/reconnect to the same durable profile.
                    assert message({'type': 'ping'}).get('ok'), 'Native Host did not recover Core'
                    current = [process.pid for process in psutil.process_iter(['exe'])
                               if process.info['exe'] and Path(process.info['exe']).resolve() == executable]
                    assert len(current) == 1 and current[0] != previous, current
                    return {'before': previous, 'after': current[0], 'unclean_shutdown': True}

                def accepted_case(name, suffix, reject=False, expect_failed=False, trigger=None, reconnect=False):
                    old_handoffs = {row['id'] for row in handoffs()}
                    before = {task['task_id'] for task in tasks()}
                    if trigger:
                        response = trigger()
                        assert response.get('ok'), response
                    else:
                        click(name)
                    def pending():
                        found = [row for row in handoffs() if row['id'] not in old_handoffs]
                        assert len(found) <= 1, found
                        return found[0] if found else None
                    handoff = wait(pending, f'{name}: desktop confirmation not queued', 35)
                    offered.add(handoff['id'])
                    assert handoff['status'] == 'pending', handoff
                    assert before == {task['task_id'] for task in tasks()}
                    if reconnect:
                        stopped = restart_host()
                        recovered = message({'type': 'handoff-status', 'handoffId': handoff['id']})
                        assert recovered.get('ok') and recovered['handoff']['id'] == handoff['id'], recovered
                        assert {row['id'] for row in handoffs()} == old_handoffs | {handoff['id']}
                        if trigger:
                            repeated = trigger()
                            assert repeated.get('ok') and repeated['handoff']['id'] == handoff['id'], repeated
                            assert {row['id'] for row in handoffs()} == old_handoffs | {handoff['id']}
                        record(name + '-host-reconnect-same-handoff', {'stopped': stopped, 'handoff_id': handoff['id']})
                    if args.restart_recovery and name in ('form', 'credential-post'):
                        retained_source = handoff['offer'].get('local_source_path')
                        retained_bytes = Path(retained_source).read_bytes() if retained_source else None
                        for kind, restart in [('extension-reload', reload_extension), ('core-restart', restart_core)]:
                            identity = restart()
                            recovered = message({'type': 'handoff-status', 'handoffId': handoff['id']})
                            assert recovered.get('ok') and recovered['handoff']['id'] == handoff['id'], recovered
                            assert recovered['handoff']['status'] == 'pending', recovered
                            if trigger:
                                repeated = trigger()
                                assert repeated.get('ok') and repeated['handoff']['id'] == handoff['id'], repeated
                            assert {row['id'] for row in handoffs()} == old_handoffs | {handoff['id']}
                            assert before == {task['task_id'] for task in tasks()}
                            if retained_source:
                                assert Path(retained_source).read_bytes() == retained_bytes, 'Original response lost during recovery'
                            record(name + '-' + kind + '-same-handoff', {**identity, 'handoff_id': handoff['id']})
                    source = Path(handoff['offer']['local_source_path']) if handoff['offer'].get('local_source_path') else None
                    source_bytes = source.read_bytes() if source else None
                    if name == 'form':
                        assert source is not None, 'POST download must retain the original browser response'
                    if source:
                        record(name + '-original-response-retained', {'bytes': len(source_bytes), 'created_events': browser_state()['events']})
                    else:
                        assert_no_browser_download(name + '-before-decision')
                    command({'kind': 'reject_handoff' if reject else 'accept_handoff',
                             'handoff_id': handoff['id'], 'filename': suffix,
                             'download_dir': '', 'trusted_ui': True,
                             'suppress_site_kind': False})
                    if reject:
                        assert before == {task['task_id'] for task in tasks()}
                        if source:
                            assert source.read_bytes() == source_bytes
                            item = next(item for item in browser_state()['items'] if Path(item['filename']).is_file() and Path(item['filename']).samefile(source))
                            assert item['state'] == 'complete', item
                            record(name + '-rejection-preserves-source', {'sha256': hashlib.sha256(source_bytes).hexdigest()})
                            extension('chrome.downloads.removeFile(arguments[0]).then(()=>chrome.downloads.erase({id:arguments[0]})).then(done)', item['id'])
                    else:
                        def completed():
                            found = [task for task in tasks() if task['task_id'] not in before]
                            created.update(task['task_id'] for task in found)
                            assert len(found) <= 1, found
                            if found and found[0]['status'] == 'failed' and not (expect_failed and source is None):
                                raise AssertionError(found[0])
                            return found[0] if found and found[0]['status'] in ['completed', 'failed'] else None
                        task = wait(completed, f'{name}: Core did not complete', 180)
                        if expect_failed and source is None:
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
                accepted_case('form', 'post-original.bin')
                assert sum(row['path'] == '/form.bin' for row in requests) == 1, 'One-use POST was replayed'
                record('POST-original-response-never-replayed', True)

                extension("chrome.storage.local.set({authorizedCookieHosts:['127.0.0.1']}).then(()=>done(true))")
                driver.switch_to.window(page)
                driver.get(page_url)
                driver.execute_script('document.cookie="hls-validation=session-only; Path=/; SameSite=Lax"')
                for method in ['GET', 'POST']:
                    name = 'credential-' + method.lower()
                    target = page_url.removesuffix('/page') + '/' + name + '.bin?signature=a%2Fb%2B%3D&n=1'
                    driver.switch_to.window(page)
                    observed = driver.execute_async_script(
                        'const [url,method,authorization,body,done]=arguments;'
                        'fetch(url,{method,headers:{Authorization:authorization,"Content-Type":"application/json"},'
                        'body:method==="POST"?new Uint8Array(body):undefined})'
                        '.then(async r=>done({status:r.status,bytes:(await r.arrayBuffer()).byteLength}))'
                        '.catch(e=>done({error:String(e)}))', target, method, AUTHORIZATION, list(POST_BODY))
                    assert observed == {'status': 200, 'bytes': len(PAYLOAD)}, observed
                    tabs = extension('chrome.tabs.query({}).then(done)')
                    tab = next(tab for tab in tabs if tab.get('url') == page_url)
                    resource = {'url': target, 'kind': 'file', 'method': method,
                                'tabId': tab['id'], 'frameId': 0, 'pageUrl': page_url,
                                'filename': name + '.bin', 'seenAt': int(time.time() * 1000)}
                    accepted_case(name, name + '.bin', reconnect=True,
                                  trigger=lambda: message({'type': 'download-now', 'resource': resource}))
                    sent = [row for row in requests if row['path'] == urlsplit(target).path + '?' + urlsplit(target).query]
                    assert len(sent) >= 2, sent
                    original_request = sent[0]
                    for row in sent:
                        assert row['method'] == method and row['authorization_matches'] and row['cookie_matches'], row
                        assert row['body_sha256'] == original_request['body_sha256'], row
                        for header in ['referer', 'origin', 'user_agent']:
                            assert row[header] == original_request[header], (header, row, original_request)
                    record(name + '-server-verified-context', {'requests': len(sent), 'signed_query': True,
                                                              'headers_and_body_equal': True})
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
                    report['extension_state'] = extension(
                        'Promise.all([chrome.storage.local.get(null),chrome.storage.session.get(null)])'
                        '.then(([local,session])=>done({local,session}))')
                if args.browser == 'firefox':
                    with contextlib.suppress(Exception):
                        driver.set_context('chrome')
                        report['browser_console'] = driver.execute_script(
                            'return Services.console.getMessageArray().map(item=>item.message)'
                            '.filter(message=>/HLS|hls|extension/i.test(message)).slice(-60)')
                        driver.set_context('content')
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
            report['http_requests'] = requests
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ('extension', 'binary', 'driver', 'report'):
        parser.add_argument('--' + flag, type=Path, required=True)
    parser.add_argument('--runtime', type=Path)
    for flag in ('isolated-engine', 'isolated-host', 'isolated-presenter'):
        parser.add_argument('--' + flag, type=Path)
    parser.add_argument('--headed', action='store_true')
    parser.add_argument('--public', action='store_true')
    parser.add_argument('--restart-recovery', action='store_true',
                        help='Also restart Firefox extension and crash/recover the isolated Core before accepting POST')
    parser.add_argument('--browser', choices=('firefox', 'edge', 'chrome'), default='firefox')
    arguments = parser.parse_args()
    components = [arguments.isolated_engine, arguments.isolated_host, arguments.isolated_presenter]
    if arguments.restart_recovery and (not all(components) or arguments.browser != 'firefox'):
        parser.error('--restart-recovery requires Firefox and all three isolated components')
    if any(components):
        if not all(components) or arguments.runtime:
            parser.error('Use all three isolated components, or --runtime, exclusively')
        with isolated_runtime(arguments) as isolation:
            result = run(arguments)
            result['isolation'] = isolation
        result['isolated_runtime_cleaned'] = not arguments.runtime.exists()
        arguments.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
    else:
        if not arguments.runtime:
            parser.error('--runtime or all three isolated components are required')
        result = run(arguments)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    raise SystemExit(0 if result['passed'] else 1)
