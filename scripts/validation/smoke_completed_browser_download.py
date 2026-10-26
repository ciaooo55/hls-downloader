"""Sandbox-only completed GET ownership with real browser, Native Host and Core.

The extension API starts each browser transfer so Firefox response blocking and
Chromium link interception cannot hide the completed-DownloadItem race.
"""
from __future__ import annotations

import argparse
from collections import Counter
import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import tempfile
import threading
import traceback

from smoke_extension_takeover import EXTENSION_ID, _cdp_evaluate, _create_driver, _debug_targets, _extension_call, _registered_host
from smoke_v7_portable_app import Pipe, wait

PAYLOAD = bytes(range(256)) * 4


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Windows Sandbox only'
    report = {'passed': False, 'browser': args.browser, 'checks': [], 'decision_transport': 'Core IPC',
              'download_start': 'browser extension downloads API',
              'background_sha256': hashlib.sha256((args.extension/'background.js').read_bytes()).hexdigest()}
    requests = Counter()
    created = set()
    offered = set()
    args.report.parent.mkdir(parents=True, exist_ok=True)

    def save():
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    def record(name, actual):
        report['checks'].append({'name': name, 'actual': actual})
        save()
        print(name, flush=True)

    def core(message):
        pipe = Pipe()
        try:
            return pipe.request(message)
        finally:
            pipe.close()

    def tasks():
        return core({'type': 'snapshot', 'request_id': 1})['tasks']

    def handoffs():
        return [json.loads(row) for row in core({'type': 'load_handoffs', 'request_id': 2})['items']]

    def command(value):
        response = core({'type': 'command', 'request_id': 3, 'command': value})
        assert response.get('type') != 'error', response
        return response

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            requests[self.path] += 1
            if requests[self.path] > 1:
                self.send_error(410, 'One-use export already consumed')
                return
            self.send_response(200)
            self.send_header('Content-Type', 'application/octet-stream')
            self.send_header('Content-Length', str(len(PAYLOAD)))
            self.end_headers()
            self.wfile.write(PAYLOAD)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    driver = None
    try:
        with tempfile.TemporaryDirectory(prefix='completed-get-', ignore_cleanup_errors=True) as temporary:
            root = Path(temporary)
            downloads = root/'downloads'
            downloads.mkdir()
            manifest = root/'host.json'
            manifest.write_text(json.dumps({
                'name': 'com.ciaooo55.hls_downloader', 'description': 'Completed GET acceptance',
                'path': str(args.runtime/'app/resources/HLSDownloaderNativeHost.exe'), 'type': 'stdio',
                **({'allowed_extensions': ['hls-downloader-store@ciaooo55.com']} if args.browser == 'firefox'
                   else {'allowed_origins': [f'chrome-extension://{EXTENSION_ID}/']}),
            }), encoding='utf-8')
            with _registered_host(manifest, args.browser):
                driver = _create_driver(args.browser, args.binary, args.extension, root/'profile', downloads, args.driver)
                driver.set_script_timeout(60)
                report['browser_version'] = driver.capabilities.get('browserVersion')
                if args.browser == 'firefox':
                    driver.set_context('chrome')
                    uuids = json.loads(driver.execute_script("return Services.prefs.getStringPref('extensions.webextensions.uuids','{}')"))
                    driver.set_context('content')
                    popup = f'moz-extension://{uuids["hls-downloader-store@ciaooo55.com"]}/popup.html'
                else:
                    popup = f'chrome-extension://{EXTENSION_ID}/popup.html'
                driver.get(popup)

                def extension(script, *values):
                    return _extension_call(driver, script, *values)

                assert extension("chrome.runtime.sendMessage({type:'ping'}).then(done)").get('ok')
                assert extension("chrome.runtime.sendMessage({type:'set-takeover-settings',enabled:true,minimumBytes:0}).then(done)").get('ok')
                extension('chrome.storage.local.set({excludedHosts:[]}).then(()=>done(true))')
                core({'type': 'store_settings', 'request_id': 4, 'values': {'legal_terms_accepted': True}})
                # 保持设置读取未完成，直到真实 DownloadItem 完成，再放行交接。
                # 不改产品构建物、请求、下载事件或 Core 回执，不依赖固定等待时间。
                if args.browser == 'firefox':
                    extension('''chrome.runtime.getBackgroundPage().then(bg=>{
                      const area=bg.browser.storage.local, get=area.get.bind(area);
                      area.get=async keys=>{const value=await get(keys);
                        const gate=bg.__qaOwnershipGate;
                        if(Array.isArray(keys)&&keys.includes('excludedHosts')&&gate){gate.hits++;await gate.promise}
                        return value};done(true)})''')

                    def gate(action):
                        return extension('''chrome.runtime.getBackgroundPage().then(bg=>{
                          if(arguments[0]==='hold'){
                            const gate={hits:0};gate.promise=new Promise(r=>gate.release=r);bg.__qaOwnershipGate=gate;done(true)
                          }else{const gate=bg.__qaOwnershipGate;bg.__qaOwnershipGate=null;gate.release();done(gate.hits)}
                        })''', action)
                else:
                    worker = wait(lambda: next((target for target in _debug_targets(driver)
                                  if target.get('type') == 'service_worker' and EXTENSION_ID in target.get('url', '')), None),
                                  'No extension worker for controlled storage delay', 15)
                    assert _cdp_evaluate(worker['webSocketDebuggerUrl'], '''(()=>{
                      const area=(globalThis.browser?.runtime?.id?browser:chrome).storage.local, get=area.get.bind(area);
                      area.get=async keys=>{const value=await get(keys);
                        const gate=globalThis.__qaOwnershipGate;
                        if(Array.isArray(keys)&&keys.includes('excludedHosts')&&gate){gate.hits++;await gate.promise}
                        return value};return true})()''') is True

                    def gate(action):
                        script = '''(()=>{const gate={hits:0};gate.promise=new Promise(r=>gate.release=r);
                                      globalThis.__qaOwnershipGate=gate;return true})()''' if action == 'hold' else '''(()=>{
                                      const gate=globalThis.__qaOwnershipGate;globalThis.__qaOwnershipGate=null;
                                      gate.release();return gate.hits})()'''
                        return _cdp_evaluate(worker['webSocketDebuggerUrl'], script)

                for name in ('accepted', 'rejected', 'source-unavailable'):
                    before_handoffs = {row['id'] for row in handoffs()}
                    before_tasks = {task['task_id'] for task in tasks()}
                    url = f'http://127.0.0.1:{server.server_port}/{name}'
                    assert gate('hold') is True
                    download_id = extension('chrome.downloads.download({url:arguments[0],filename:arguments[1],saveAs:false}).then(done)', url, name+'.bin')
                    def browser_completed():
                        items = extension('chrome.downloads.search({id:arguments[0]}).then(done)', download_id)
                        return items[0] if items and items[0]['state'] == 'complete' else None
                    current = wait(browser_completed, 'Browser transfer did not complete while settings were held', 40)
                    hits = gate('release')
                    assert hits > 0, 'Controlled storage gate did not intercept the real handler'
                    record(name+': browser completed before handler continued', {'state': current['state'], 'settings_reads_held': hits})
                    handoff = wait(lambda: next((row for row in handoffs() if row['id'] not in before_handoffs), None), 'No completed-file offer', 40)
                    offered.add(handoff['id'])
                    assert handoff['status'] == 'pending', handoff
                    source_path = handoff['offer'].get('local_source_path')
                    assert source_path, 'Completed GET was replayed as URL: '+str(handoff)
                    source = Path(source_path)
                    assert source.read_bytes() == PAYLOAD
                    assert requests['/'+name] == 1, requests
                    record(name+': local source retained before decision', {'requests': requests['/'+name], 'sha256': hashlib.sha256(source.read_bytes()).hexdigest()})

                    backup = None
                    try:
                        if name == 'source-unavailable':
                            backup = source.with_suffix('.retained')
                            source.rename(backup)
                        decision = {'kind': 'reject_handoff' if name == 'rejected' else 'accept_handoff',
                                    'handoff_id': handoff['id'], 'filename': name+'.bin',
                                    'download_dir': '', 'suppress_site_kind': False}
                        if name == 'source-unavailable':
                            failed = core({'type': 'command', 'request_id': 3, 'command': decision})
                            assert failed.get('type') == 'error' and '本地源文件' in failed.get('message', ''), failed
                            assert before_tasks == {task['task_id'] for task in tasks()}
                            record('missing source rejected before task creation', failed)
                        else:
                            command(decision)
                        if name == 'rejected':
                            assert before_tasks == {task['task_id'] for task in tasks()}
                        elif name == 'accepted':
                            def finished():
                                found = [task for task in tasks() if task['task_id'] not in before_tasks]
                                created.update(task['task_id'] for task in found)
                                assert len(found) <= 1, found
                                return found[0] if found and found[0]['status'] in ('completed', 'failed') else None
                            task = wait(finished, 'Local copy did not finish', 60)
                            assert task['status'] == 'completed', task
                            assert Path(task['output_path']).read_bytes() == PAYLOAD
                            wait(lambda: not source.exists(), 'Source not cleaned after copy', 20)
                            wait(lambda: not extension('chrome.downloads.search({id:arguments[0]}).then(done)', download_id), 'Completed browser record not removed', 20)
                        assert requests['/'+name] == 1, requests
                    finally:
                        if backup and backup.exists():
                            backup.rename(source)
                    if name != 'accepted':
                        assert source.read_bytes() == PAYLOAD
                        items = extension('chrome.downloads.search({id:arguments[0]}).then(done)', download_id)
                        assert len(items) == 1 and items[0]['state'] == 'complete', items
                        # 仅清理本轮沙盒夹具，不能把它计为产品已自动清理。
                        extension('chrome.downloads.removeFile(arguments[0]).then(()=>chrome.downloads.erase({id:arguments[0]})).then(done)', download_id)
                    record(name+': no URL replay, correct source ownership', {'requests': requests['/'+name]})
                report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
        report['traceback'] = traceback.format_exc()
    finally:
        if driver:
            with contextlib.suppress(Exception):
                driver.quit()
        with contextlib.suppress(Exception):
            for row in handoffs():
                if row['id'] in offered and row['status'] == 'pending':
                    command({'kind': 'reject_handoff', 'handoff_id': row['id'], 'suppress_site_kind': False})
        for task_id in created:
            with contextlib.suppress(Exception):
                command({'kind': 'task_action', 'task_id': task_id, 'action': 'delete_files'})
        server.shutdown()
        server.server_close()
        report['requests'] = dict(requests)
        save()
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--browser', choices=('firefox', 'edge'), required=True)
    for flag in ('runtime', 'extension', 'binary', 'driver', 'report'):
        parser.add_argument('--'+flag, type=Path, required=True)
    result = run(parser.parse_args())
    print(json.dumps({'passed': result['passed'], 'error': result.get('error')}, ensure_ascii=False))
    raise SystemExit(0 if result['passed'] else 1)
