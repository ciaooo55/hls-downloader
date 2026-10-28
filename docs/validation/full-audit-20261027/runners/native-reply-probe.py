"""Headless navigation + actual Host/Core; no desktop pointer, keyboard or UI input."""
import argparse
import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import traceback

from smoke_extension_takeover import EXTENSION_ID, _create_driver, _extension_call, _registered_host
from smoke_v7_portable_app import Pipe, wait

assert os.environ.get('USERNAME') == 'WDAGUtilityAccount', 'Windows Sandbox only'
PAYLOAD = bytes(range(256)) * 4096


def core(message):
    pipe = Pipe()
    try:
        result = pipe.request(dict(message, request_id=1))
        assert result.get('type') != 'error', result
        return result
    finally:
        pipe.close()


def run(args):
    result = {'passed': False, 'desktop_input_used': False, 'browser': args.browser,
              'background_sha256': hashlib.sha256((args.extension/'background.js').read_bytes()).hexdigest()}
    requests = []
    driver = None
    created = set()
    offered = set()

    def checkpoint(phase):
        result['phase'] = phase
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2)+'\n', encoding='utf8')

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            requests.append({'path': self.path, 'range': self.headers.get('Range')})
            if self.path == '/favicon.ico':
                self.send_error(404)
                return
            if self.path == '/page':
                body = b'<!doctype html><title>Native reply recovery</title>'
                mime = 'text/html'
            else:
                body, mime = PAYLOAD, 'application/octet-stream'
            start, end = 0, len(body)-1
            if self.headers.get('Range'):
                value = self.headers['Range'].removeprefix('bytes=').split('-')
                start, end = int(value[0]), int(value[1]) if value[1] else end
                self.send_response(206)
                self.send_header('Content-Range', f'bytes {start}-{end}/{len(body)}')
            else:
                self.send_response(200)
            self.send_header('Content-Type', mime)
            self.send_header('Content-Length', str(end-start+1))
            self.send_header('Accept-Ranges', 'bytes')
            if self.path != '/page':
                self.send_header('Content-Disposition', 'attachment; filename="recovery.bin"')
            self.end_headers()
            with contextlib.suppress(ConnectionError):
                for offset in range(start, end+1, 8192):
                    self.wfile.write(body[offset:min(offset+8192, end+1)])
                    if self.path != '/page':
                        time.sleep(.02)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        # Core repairs its normal Native Host registration on startup. Let that
        # finish before registering the proxy, otherwise retries bypass it.
        engine_path = Path(os.environ['HLS_REPLY_REAL_HOST']).with_name('HLSDownloaderEngine.exe')
        try:
            core({'type': 'snapshot'})
            engine = None
        except OSError:
            engine = subprocess.Popen([str(engine_path)], creationflags=subprocess.CREATE_NO_WINDOW,
                                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

        def core_ready():
            if engine is not None and engine.poll() is not None:
                raise RuntimeError(f'Core exited during setup: {engine.returncode}')
            try:
                return core({'type': 'snapshot'})
            except OSError:
                return None

        wait(core_ready, 'Core startup before proxy registration', 30)
        with tempfile.TemporaryDirectory(dir=args.state, prefix='browser-') as temporary:
            root = Path(temporary)
            downloads = root/'downloads'
            downloads.mkdir()
            manifest = root/'host.json'
            manifest.write_text(json.dumps({
                'name': 'com.ciaooo55.hls_downloader', 'description': 'Sandbox reply fault injection',
                'path': str(args.proxy), 'type': 'stdio',
                **({'allowed_extensions': ['hls-downloader-store@ciaooo55.com']} if args.browser == 'firefox'
                   else {'allowed_origins': [f'chrome-extension://{EXTENSION_ID}/']}),
            }), encoding='utf8')
            with _registered_host(manifest, args.browser), contextlib.ExitStack() as cleanup:
                checkpoint('browser-startup')
                driver = _create_driver(args.browser, args.binary, args.extension, root/'profile', downloads, args.driver)
                cleanup.callback(driver.quit)
                driver.set_script_timeout(40)
                driver.set_page_load_timeout(30)
                result['browser_version'] = driver.capabilities.get('browserVersion')
                page_url = f'http://127.0.0.1:{server.server_port}/page'
                driver.get(page_url)
                page = driver.current_window_handle
                if args.browser == 'firefox':
                    driver.set_context('chrome')
                    uuids = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
                    driver.set_context('content')
                    popup = f'moz-extension://{uuids["hls-downloader-store@ciaooo55.com"]}/popup.html'
                else:
                    popup = f'chrome-extension://{EXTENSION_ID}/popup.html'
                driver.switch_to.new_window('tab')
                inspector = driver.current_window_handle
                driver.get(popup)
                driver.execute_script('window.createdDownloads=[]; chrome.downloads.onCreated.addListener(d=>createdDownloads.push({id:d.id,url:d.url}));')

                def extension(script, *values):
                    driver.switch_to.window(inspector)
                    return _extension_call(driver, script, *values)

                def capture_browser():
                    with contextlib.suppress(Exception):
                        result['browser_state_at_close'] = extension(
                            'Promise.all([chrome.downloads.search({}),chrome.storage.session.get(null)]).then(([items,stored])=>'
                            'done({items,events:createdDownloads,errors:Object.fromEntries(Object.entries(stored).filter(([key])=>key.startsWith("download-error:")))}))')

                cleanup.callback(capture_browser)

                result['ping'] = extension('chrome.runtime.sendMessage({type:"ping"}).then(done)')
                checkpoint('host-ping')
                assert result['ping'].get('ok'), result['ping']
                core({'type': 'store_settings', 'values': {'legal_terms_accepted': True,
                    'download_dir': str(args.state/'output'), 'download_progress_window_enabled': False,
                    'download_complete_popup_enabled': False, 'av_scan_enabled': False}})
                before = {task['task_id'] for task in core({'type': 'snapshot'})['tasks']}
                old_handoffs = {json.loads(row)['id'] for row in core({'type': 'load_handoffs'})['items']}
                driver.switch_to.window(page)
                driver.execute_script('location.assign(arguments[0])', f'http://127.0.0.1:{server.server_port}/recovery.bin')

                def pending():
                    found = [json.loads(row) for row in core({'type': 'load_handoffs'})['items']
                             if json.loads(row)['id'] not in old_handoffs]
                    offered.update(row['id'] for row in found)
                    assert len(found) <= 1, found
                    return found[0] if found else None

                handoff = wait(pending, 'offer not persisted', 40)
                result['persisted_handoff'] = handoff
                checkpoint('offer-persisted')
                assert handoff['status'] == 'pending', handoff

                def recovered():
                    log = args.state/'offers.jsonl'
                    rows = [json.loads(row) for row in log.read_text().splitlines()] if log.exists() else []
                    return rows if len(rows) >= 2 else None

                rows = wait(recovered, 'Native Bridge did not retry the acknowledged pipe error', 15)
                assert len(rows) == 2 and [row['injected'] for row in rows] == [True, False], rows
                assert len({row['client_request_id'] for row in rows}) == 1, rows
                assert len({row['request_sha256'] for row in rows}) == 1, rows
                assert {row['handoff_id'] for row in rows} == {handoff['id']}, rows
                assert {task['task_id'] for task in core({'type': 'snapshot'})['tasks']} == before
                core({'type': 'command', 'command': {'kind': 'accept_handoff', 'handoff_id': handoff['id'],
                    'filename': 'recovery.bin', 'download_dir': str(args.state/'output'), 'trusted_ui': True}})

                def completed():
                    found = [task for task in core({'type': 'snapshot'})['tasks'] if task['task_id'] not in before]
                    created.update(task['task_id'] for task in found)
                    assert len(found) <= 1, found
                    if found and found[0]['status'] == 'failed':
                        raise AssertionError(found[0])
                    return found[0] if found and found[0]['status'] == 'completed' else None

                task = wait(completed, 'download did not complete', 60)
                digest = hashlib.sha256(Path(task['output_path']).read_bytes()).hexdigest()
                assert digest == hashlib.sha256(PAYLOAD).hexdigest()

                def browser_settled():
                    state = extension('Promise.all([chrome.downloads.search({}),Promise.resolve(createdDownloads)]).then(([items,events])=>done({items,events}))')
                    return state if not state['items'] else None

                state = wait(browser_settled, 'browser transfer not released', 15)
                if args.browser == 'firefox':
                    assert not state['events'], state
                else:
                    assert len(state['events']) == 1, state
                result.update(passed=True, offers=rows, task_count=1, handoff_count=1,
                              output_sha256=digest, browser_state=state,
                              zero_download_item=not state['events'], server_requests=requests)
    except Exception:
        result['error'] = traceback.format_exc()
        result['server_requests'] = requests
    finally:
        if (args.state/'offers.jsonl').exists():
            result['offers_observed'] = [json.loads(row) for row in (args.state/'offers.jsonl').read_text().splitlines()]
        result['proxy_diagnostics'] = {
            name: (args.state/name).read_text(encoding='utf8', errors='replace')
            for name in ['proxy-errors.txt', 'host-stderr.txt', 'injected-reply.json'] if (args.state/name).exists()
        }
        result['proxy_trace'] = {
            path.name: [json.loads(row) for row in path.read_text().splitlines()]
            for path in args.state.glob('proxy-*.jsonl')
        }
        for handoff_id in offered:
            with contextlib.suppress(Exception):
                core({'type': 'command', 'command': {'kind': 'reject_handoff', 'handoff_id': handoff_id}})
        for task_id in created:
            with contextlib.suppress(Exception):
                core({'type': 'command', 'command': {'kind': 'task_action', 'task_id': task_id, 'action': 'delete_files'}})
        server.shutdown()
        server.server_close()
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2)+'\n', encoding='utf8')
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--browser', choices=['firefox', 'edge'], required=True)
    for name in ['extension', 'binary', 'driver', 'proxy', 'state', 'report']:
        parser.add_argument('--'+name, type=Path, required=True)
    args = parser.parse_args()
    args.state.mkdir(parents=True, exist_ok=True)
    raise SystemExit(0 if run(args)['passed'] else 1)
