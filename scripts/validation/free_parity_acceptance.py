"""Isolated acceptance for the free CLI, crawl/sync and bounded ZIP preview.

Uses local origins only. This is a development component check, not a claim that
the installer, browser or native workbench matrices passed for these binaries.
"""
from __future__ import annotations

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
import traceback
import uuid
from urllib.parse import urlsplit
import zipfile

from v7_core_ipc import connect, free_port, send_frame as send
from validation_workspace import test_tmp


class Origin(ThreadingHTTPServer):
    daemon_threads = True
    def handle_error(self, *_):
        pass


def archive(entries):
    body = io.BytesIO()
    with zipfile.ZipFile(body, 'w', compression=zipfile.ZIP_STORED) as z:
        for name, value in entries:
            z.writestr(name, value)
    return body.getvalue()


def zip64_empty():
    record = struct.pack('<4sQHHIIQQQQ', b'PK\x06\x06', 44, 45, 45, 0, 0, 0, 0, 0, 0)
    locator = struct.pack('<4sIQI', b'PK\x06\x07', 0, 0, 1)
    end = struct.pack('<4sHHHHIIH', b'PK\x05\x06', 0, 0, 65535, 65535, 0xffffffff, 0xffffffff, 0)
    return record + locator + end


def run(args):
    import psutil
    args.work.mkdir(parents=True, exist_ok=True)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    os.chdir(args.work)
    resources = args.resources.resolve()
    cli = resources / 'HLSDownloaderCli.exe'
    engine = resources / 'HLSDownloaderEngine.exe'
    result = {'passed': False, 'finished': False, 'scope': 'development component acceptance',
              'checks': [], 'components': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in [cli, engine]}}
    requests = []
    payload = bytes(range(256)) * 32768
    large_zip = archive([('目录/文件.txt', b'hello'), ('payload.bin', payload)])
    central_zip = archive([(f'entry-{i:05}.txt', b'') for i in range(2200)])
    state = {'version': 1, 'broken': False, 'outside': 0, 'changing': 0}
    gated_request = threading.Event()
    periodic_get = threading.Event()
    origins = []
    core_port, origin_port, outside_port = free_port(), free_port(), free_port()
    base = f'http://127.0.0.1:{origin_port}'

    class Handler(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'
        def log_message(self, *_):
            pass
        def do_HEAD(self):
            self.handle_body(False)
        def do_GET(self):
            self.handle_body(True)
        def handle_body(self, include):
            path = urlsplit(self.path).path
            record = {'path': path, 'method': self.command, 'range': self.headers.get('Range'),
                      'if_none_match': self.headers.get('If-None-Match'), 'bytes': 0,
                      'user_agent': self.headers.get('User-Agent'), 'referer': self.headers.get('Referer'),
                      'cookie': self.headers.get('Cookie'), 'crawl_header': self.headers.get('X-Crawl-Fixture')}
            requests.append(record)
            if path.startswith('/gated/'):
                gated_request.set()
            if path == '/gated/asset.bin' and include and state['version'] == 2:
                periodic_get.set()
            if self.server.server_port == outside_port:
                state['outside'] += 1
            if path == '/cross':
                self.send_response(302); self.send_header('Location', f'http://127.0.0.1:{outside_port}/outside'); self.send_header('Content-Length', '0'); self.end_headers(); return
            bodies = {
                '/site/index.html': b'<base href="/site/"><a href="next.html">next</a><a href="./a/../index.html#loop">loop</a><link href="style.css"><img src="image.bin">',
                '/site/next.html': b'<a href="index.html">back</a><img src="image.bin">',
                '/site/style.css': b'.x{background:url("image.bin")}',
                '/site/image.bin': b'image-version-' + str(state['version']).encode(),
                '/sync/index.html': b'<a href="asset.bin">asset</a>',
                '/sync/asset.bin': (b'original-' if state['version'] == 1 else b'updated-') * 8192,
                '/gated/index.html': b'<a href="asset.bin">asset</a>',
                '/gated/asset.bin': (b'original' if state['version'] == 1 else b'updated!') * 8192,
                '/context.bin': b'context-fixture' * 4096,
                '/fixture.bin': payload,
                '/large.zip': large_zip,
                '/central.zip': central_zip,
                '/changing.zip': central_zip,
                '/norange.zip': large_zip,
            }
            data = bodies.get(path, b'not found')
            etag = f'"fixture-{state["version"]}"'
            if path == '/changing.zip':
                state['changing'] += 1
                etag = f'"changed-{state["changing"]}"'
            status = 200 if path in bodies else 404
            if path == '/sync/asset.bin' and state['broken'] and include:
                status = 503; data = b'fixture failure'
            if self.headers.get('If-None-Match') == etag:
                self.send_response(304); self.send_header('ETag', etag); self.send_header('Content-Length', '0'); self.end_headers(); return
            start, end = 0, len(data) - 1
            range_value = self.headers.get('Range')
            if status == 200 and range_value and path != '/norange.zip':
                a, b = range_value.removeprefix('bytes=').split('-', 1)
                if not a:
                    start = max(0, len(data) - int(b))
                else:
                    start, end = int(a), min(int(b) if b else end, end)
                status = 206
            self.send_response(status)
            self.send_header('Content-Length', str(end - start + 1))
            self.send_header('ETag', etag)
            self.send_header('Content-Type', 'text/html' if path.endswith('.html') else 'application/octet-stream')
            self.send_header('Accept-Ranges', 'bytes')
            if status == 206:
                self.send_header('Content-Range', f'bytes {start}-{end}/{len(data)}')
            self.end_headers()
            if not include:
                return
            for offset in range(start, end + 1, 32768):
                chunk = data[offset:min(offset + 32768, end + 1)]
                try:
                    self.wfile.write(chunk); self.wfile.flush(); record['bytes'] += len(chunk)
                except OSError:
                    break
                if path == '/fixture.bin':
                    time.sleep(.004)

    def save():
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
    def check(name, fn):
        result['phase'] = name; save()
        started = time.monotonic()
        try:
            evidence = fn()
            result['checks'].append({'name': name, 'passed': True, 'seconds': round(time.monotonic()-started, 3), 'evidence': evidence})
        except Exception as e:
            result['checks'].append({'name': name, 'passed': False, 'error': repr(e), 'traceback': traceback.format_exc()})
        save()
    def invoke(*arguments, code=0, input_text=None):
        args.report.with_suffix('.invocation.json').write_text(json.dumps({'arguments':list(map(str,arguments)),'started':time.time()}),encoding='utf-8')
        proc = subprocess.run([str(cli), '--json', *map(str, arguments)], input=input_text.encode('utf-8') if input_text is not None else None, capture_output=True, timeout=40)
        assert proc.returncode == code, (arguments, proc.returncode, proc.stdout.decode('utf-8', 'replace'), proc.stderr.decode('utf-8', 'replace'))
        return json.loads((proc.stdout if code == 0 else proc.stderr).decode('utf-8'))
    def ipc(message):
        with connect(core_port) as channel:
            return send(channel, message)
    def until(fn, predicate, seconds=40):
        deadline = time.monotonic() + seconds
        while True:
            value = fn()
            if predicate(value): return value
            if time.monotonic() >= deadline: raise AssertionError(value)
            time.sleep(.2)
    def task(task_id):
        return next(t for t in ipc({'type': 'snapshot', 'request_id': 1})['tasks'] if t['task_id'] == task_id)
    def project(project_id):
        return invoke('crawl', 'get', project_id)['projects'][0]
    def create_project(entry, **options):
        spec = args.work / 'crawl-spec.json'
        spec.write_text(json.dumps(dict(entry_url=base+entry, output_dir=str(args.work/'downloads'), **options)), encoding='utf-8')
        return invoke('crawl', 'create', '--spec', spec)['projects'][0]
    def owned_cores():
        return [p for p in psutil.process_iter(['exe']) if p.info['exe'] and Path(p.info['exe']) == engine]
    def stop_core():
        for p in owned_cores(): p.kill(); p.wait(timeout=10)

    try:
        os.environ.update(HLS_VM_ACCEPTANCE='1', HLS_V7_DATA_DIR=str(args.work/'data'),
                          HLS_V7_DOWNLOAD_DIR=str(args.work/'downloads'), HLS_V7_CORE_TCP='1', HLS_V7_CORE_BIND=f'127.0.0.1:{core_port}',
                          HLS_V7_PIPE=rf'\\.\pipe\HLSDownloader.FreeParity.{uuid.uuid4().hex}', TEMP=str(args.work/'tmp'), TMP=str(args.work/'tmp'))
        (args.work/'tmp').mkdir(exist_ok=True)
        for port in [origin_port, outside_port]:
            origin = Origin(('127.0.0.1', port), Handler); origins.append(origin)
            threading.Thread(target=origin.serve_forever, daemon=True).start()

        def startup():
            assert invoke('list')['tasks'] == []
            for _ in range(3): invoke('list')
            assert len(owned_cores()) == 1
            response = ipc({'type':'store_settings', 'request_id':1, 'values':{
                'legal_terms_accepted':True, 'legal_terms_version':'2026-08-06-cn-1',
                'download_dir':str(args.work/'downloads'), 'temp_dir':str(args.work/'tmp'),
                'download_progress_window_enabled':False, 'download_complete_popup_enabled':False,
                'default_concurrency':2, 'queue_max_active':2}})
            assert response['type'] == 'settings', response
            return {'core_count':1, 'workbench_started':False}
        check('cli-starts-one-core-without-workbench', startup)

        def cli_lifecycle():
            item=invoke('add',base+'/fixture.bin','--name','中文 文件.bin','--dir',args.work/'downloads'/'中文 空格','--paused')['tasks'][0]
            task_id=item['task_id']; assert item['status']=='paused'
            assert task(task_id)['downloaded_bytes']==0
            invoke('resume', task_id)
            until(lambda:task(task_id),lambda t:t['downloaded_bytes']>0)
            invoke('pause',task_id)
            until(lambda:task(task_id),lambda t:t['status']=='paused' and t['active_workers']==0)
            stop_core(); invoke('list')
            assert invoke('show',task_id)['tasks'][0]['status']=='paused'
            invoke('resume',task_id)
            complete=until(lambda:task(task_id),lambda t:t['status'] in ['completed','failed'])
            assert complete['status']=='completed',complete
            output=Path(complete['output_path']); digest=hashlib.sha256(output.read_bytes()).hexdigest()
            assert digest==hashlib.sha256(payload).hexdigest()
            export=args.work/'任务 导出.json'; invoke('export',task_id,'--output',export)
            assert export.is_file(); invoke('export',task_id,'--output',export,code=5)
            invoke('show','missing-task',code=4); invoke('invalid-command',code=2)
            invoke('remove',task_id); assert output.is_file()
            imported=invoke('import',export); assert imported['tasks'],imported
            return {'sha256':digest,'unicode_path':str(output),'preserved_on_remove':True,'restart_restored_paused':True}
        check('cli-lifecycle-recovery-export-import',cli_lifecycle)

        def cli_queue():
            assert invoke('queue','stop','default')['enabled'] is False
            try:
                # Same-URL admission intentionally reuses the previous imported task.
                item=invoke('add',base+'/fixture.bin?queue-control','--name','queue-control.bin')['tasks'][0]
                current=task(item['task_id'])
                assert current['downloaded_bytes']==0 and current['active_workers']==0,current
            finally:
                assert invoke('queue','start','default')['enabled'] is True
            done=until(lambda:task(item['task_id']),lambda t:t['status'] in ['completed','failed'])
            assert done['status']=='completed',done
            output=Path(done['output_path']);assert hashlib.sha256(output.read_bytes()).digest()==hashlib.sha256(payload).digest()
            for format in ['csv','urls']:
                exported=args.work/('queue-export.'+format)
                invoke('export',item['task_id'],'--format',format,'--output',exported)
                assert exported.stat().st_size>0
            invoke('remove',item['task_id'],'--delete-files');assert not output.exists()
            invoke('queue','start','nonexistent-queue',code=4)
            return {'queued_without_transfer':True,'started_and_sha256_correct':True,'csv_urls_export':True,'explicit_delete_files':True}
        check('cli-queue-controls-and-explicit-file-deletion',cli_queue)

        def cli_context():
            context = dict(url=base+'/context.bin',headers={'Cookie':'cli-fixture=local-only','X-Crawl-Fixture':'stdin-fixture'})
            created = invoke('add','--context','-','--name','context.bin',input_text=json.dumps(context))
            assert 'cli-fixture' not in json.dumps(created) and context['url'] not in json.dumps(created)
            item = created['tasks'][0]
            done = until(lambda:task(item['task_id']),lambda t:t['status'] in ['completed','failed'])
            assert done['status']=='completed',done
            assert Path(done['output_path']).read_bytes()==b'context-fixture'*4096
            observed = [r for r in requests if r['path']=='/context.bin']
            assert observed and all(r['cookie']=='cli-fixture=local-only' and r['crawl_header']=='stdin-fixture' for r in observed),observed
            return {'stdin_context_applied':True,'default_result_redacted':True}
        check('cli-stdin-context-and-default-redaction',cli_context)

        def zip_preview():
            fixtures={'normal.zip':large_zip,'empty.zip':archive([]),'zip64.zip':zip64_empty(),'damaged.zip':b'not a zip'}
            for name,data in fixtures.items(): (args.work/name).write_bytes(data)
            local=invoke('zip',args.work/'normal.zip'); assert local['entries'][0]['path']=='目录/文件.txt'
            assert invoke('zip',args.work/'empty.zip')['entries']==[]
            assert invoke('zip',args.work/'zip64.zip')['zip64']
            invoke('zip',args.work/'damaged.zip',code=4)
            over_limit = args.work/'over-entry-limit.zip'
            over_limit.write_bytes(archive([(f'{i}.txt',b'') for i in range(10001)]))
            invoke('zip',over_limit,code=4)
            start=len(requests); remote=invoke('zip',base+'/large.zip')
            assert remote['entries']==local['entries'] and remote['metadata_bytes_read']<100000
            large_requests=[r for r in requests[start:] if r['path']=='/large.zip']; assert large_requests and all(r['range'] for r in large_requests)
            directory=invoke('zip',base+'/central.zip'); assert len(directory['entries'])==2200
            assert directory['metadata_bytes_read']<400000
            invoke('zip',base+'/changing.zip',code=4)
            invoke('zip',base+'/norange.zip',code=4)
            return {'archive_bytes':len(large_zip),'metadata_bytes':remote['metadata_bytes_read'],'remote_requests':large_requests,'central_directory_entries':2200}
        check('zip-local-remote-bounded-and-resource-change',zip_preview)

        def discovery_offline():
            p=create_project('/site/index.html',offline=True,max_pages=10)
            p=until(lambda:project(p['id']),lambda p:p['status'] in ['ready','paused'])
            assert p['status']=='ready',p
            assert p['visited_pages']==2 and len(p['resources'])==4,p
            assert all(not r['task_id'] and not r['saved'] for r in p['resources'])
            invoke('crawl','pause',p['id']); stop_core(); invoke('list')
            assert project(p['id'])['status']=='paused'
            invoke('crawl','resume',p['id']); invoke('crawl','enqueue',p['id'])
            p=until(lambda:project(p['id']),lambda p:all(r['saved'] for r in p['resources']),seconds=60)
            # Rewrite follows publication, so poll the file rather than assuming
            # the persisted saved flag also means the next tick already ran.
            image=next(r for r in p['resources'] if r['url'].endswith('image.bin'))
            html=next(r for r in p['resources'] if r['url'].endswith('index.html'))
            target=Path(p['spec']['output_dir'])/html['local_name']
            until(lambda:target.read_text(encoding='utf-8'),lambda t:image['local_name'] in t and '<base href="./">' in t)
            assert len({r['url'] for r in p['resources']})==4
            return {'id':p['id'],'pages':2,'resources':4,'selection_required':True,'paused_restart':True,'rewritten_html':target.read_text(encoding='utf-8')}
        check('crawl-loop-boundaries-preview-recovery-offline',discovery_offline)

        def cross_origin():
            p=create_project('/cross')
            p=until(lambda:project(p['id']),lambda p:p['status']=='paused')
            assert state['outside']==0, requests
            return {'status':p['status'],'reason':p['reason'],'outside_requests':0}
        check('crawl-redirect-blocked-before-cross-origin-request',cross_origin)

        def sync():
            state.update(version=1,broken=False)
            p=create_project('/sync/index.html')
            p=until(lambda:project(p['id']),lambda p:p['status']=='ready')
            assert len(p['resources'])==1,p
            invoke('crawl','enqueue',p['id'])
            p=until(lambda:project(p['id']),lambda p:p['resources'][0]['saved'])
            target=Path(p['spec']['output_dir'])/p['resources'][0]['local_name']
            old=target.read_bytes(); assert old==b'original-'*8192
            before=len([r for r in requests if r['path']=='/sync/asset.bin' and r['method']=='GET'])
            invoke('crawl','sync',p['id'])
            until(lambda:project(p['id']),lambda q:q['resources'][0]['last_checked_ms']>p['resources'][0]['last_checked_ms'])
            after=len([r for r in requests if r['path']=='/sync/asset.bin' and r['method']=='GET'])
            assert before==after
            state.update(version=2,broken=True); invoke('crawl','sync',p['id'])
            p=until(lambda:project(p['id']),lambda q:bool(q['resources'][0]['error']))
            assert target.read_bytes()==old
            state['broken']=False
            task_id=p['resources'][0]['task_id']; invoke('retry',task_id); invoke('crawl','sync',p['id'])
            until(lambda:target.read_bytes(),lambda b:b==b'updated-'*8192,seconds=70)
            return {'unchanged_get_count':after-before,'failed_update_preserved_old':True,'successful_update_sha256':hashlib.sha256(target.read_bytes()).hexdigest()}
        check('sync-etag-no-repeat-failure-preserves-atomic-update',sync)

        def crawl_limits():
            before = len(requests)
            p = create_project('/site/index.html', max_pages=1)
            p = until(lambda:project(p['id']), lambda q:q['status']=='paused')
            assert p['visited_pages']==1 and '上限' in p['reason'], p
            assert not any(r['path']=='/site/next.html' for r in requests[before:])
            invoke('crawl','resume',p['id'],code=4)
            before = len(requests)
            depth = create_project('/site/index.html', offline=True, max_depth=0)
            depth = until(lambda:project(depth['id']), lambda q:q['status']=='ready')
            assert depth['visited_pages']==1 and not any(r['path']=='/site/next.html' for r in requests[before:])
            return {'page_limit_paused':True,'limit_resume_refused':True,'depth_boundary_preserved':True}
        check('crawl-page-and-depth-limits',crawl_limits)

        def scheduled_context_and_periodic_sync():
            state.update(version=1,broken=False)
            original = ipc({'type':'load_settings','request_id':1})
            profiles = list(original['queue_profiles'])
            minutes = time.localtime().tm_hour*60 + time.localtime().tm_min
            clock = lambda offset:f'{((minutes+offset)//60)%24:02}:{(minutes+offset)%60:02}'
            queue = dict(id='crawl-gated',name='Crawl acceptance',enabled=True,max_active=1,
                         speed_limit_kib=64,schedule_enabled=True,start_time=clock(120),stop_time=clock(180))
            try:
                response = ipc({'type':'store_settings','request_id':1,'values':{
                    'queue_profiles':profiles+[queue], 'download_speed_limit_kib':128,
                    'site_rules':json.dumps([dict(host='127.0.0.1',enabled=True,user_agent='CrawlAcceptance/1',referer=base+'/reference')])}})
                assert response['type']=='settings' and response['speed_limit_kib']==128,response
                response = ipc({'type':'set_site_rule_credential','request_id':1,'host':'127.0.0.1',
                                'cookie':'fixture=local-only','request_headers':{'X-Crawl-Fixture':'local-only'}})
                assert response['type']=='settings',response
                gated_request.clear()
                p = create_project('/gated/index.html',queue_id=queue['id'],sync_interval_secs=60)
                assert not gated_request.wait(2.5), 'discovery ignored the closed queue window'
                queue['schedule_enabled'] = False
                response = ipc({'type':'store_settings','request_id':1,'values':{'queue_profiles':profiles+[queue]}})
                assert response['type']=='settings',response
                p = until(lambda:project(p['id']),lambda q:q['status']=='ready')
                invoke('crawl','enqueue',p['id'])
                p = until(lambda:project(p['id']),lambda q:all(r['saved'] for r in q['resources']))
                assert len(p['resources'])==1,p
                output = Path(p['spec']['output_dir'])/p['resources'][0]['local_name']
                assert output.read_bytes()==b'original'*8192
                task_snapshot = task(p['resources'][0]['task_id'])
                assert task_snapshot['queue_id']==queue['id'],task_snapshot
                observed = [r for r in requests if r['path'].startswith('/gated/')]
                assert {r['method'] for r in observed} >= {'GET','HEAD'},observed
                assert all(r['user_agent']=='CrawlAcceptance/1' and r['referer']==base+'/reference'
                           and r['cookie']=='fixture=local-only' and r['crawl_header']=='local-only' for r in observed),observed
                periodic_get.clear()
                state['version'] = 2
                assert periodic_get.wait(75), 'periodic sync did not fetch changed resource'
                until(lambda:output.read_bytes(),lambda b:b==b'updated!'*8192,seconds=15)
                invoke('crawl','pause',p['id'])
                return {'closed_window_zero_requests':True,'request_context_on_page_head_and_file':True,
                        'queue_assignment':queue['id'],'configured_global_queue_kib':[128,64],
                        'periodic_without_manual_sync':True,'sha256':hashlib.sha256(output.read_bytes()).hexdigest()}
            finally:
                ipc({'type':'store_settings','request_id':1,'values':{'queue_profiles':profiles,
                     'download_speed_limit_kib':original['speed_limit_kib'],'site_rules':original['site_rules']}})
                ipc({'type':'set_site_rule_credential','request_id':1,'host':'127.0.0.1','clear':True})
        check('crawl-queue-window-context-and-periodic-sync',scheduled_context_and_periodic_sync)

        if args.unit_tests:
            def units():
                filters=['crawl::tests::','zip_preview::tests::','store::tests::schema_six_migration_preserves_tasks_and_settings',
                         'http_engine::tests::active_split_preserves_reserved_read_and_never_overlaps_tail']
                records=[]
                for test in filters:
                    proc=subprocess.run([str(args.unit_tests),test,'--nocapture'],capture_output=True,timeout=30)
                    record={'filter':test,'exit_code':proc.returncode,'stdout':proc.stdout.decode('utf-8','replace'),'stderr':proc.stderr.decode('utf-8','replace')}
                    records.append(record)
                assert all(r['exit_code']==0 and '0 passed' not in r['stdout'] for r in records),records
                return records
            check('targeted-rust-tests-in-sandbox',units)
        result['passed']=bool(result['checks']) and all(c['passed'] for c in result['checks'])
    except Exception as e:
        result['error']=repr(e); result['traceback']=traceback.format_exc()
    finally:
        stop_core()
        for origin in origins: origin.shutdown(); origin.server_close()
        result['requests']=requests; result['finished']=True; save()
    return 0 if result['passed'] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--resources', type=Path)
    parser.add_argument('--work', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--unit-tests', type=Path)
    parser.add_argument('--isolated-engine', type=Path)
    parser.add_argument('--isolated-cli', type=Path)
    args = parser.parse_args()
    args.report = args.report.resolve()
    if args.unit_tests:
        args.unit_tests = args.unit_tests.resolve()
    if args.isolated_engine or args.isolated_cli:
        if not all((args.isolated_engine, args.isolated_cli)) or args.resources or args.work:
            parser.error('Use both isolated components, or Sandbox resources/work, exclusively')
        workspace = Path(__file__).resolve().parents[2]
        if not args.report.is_relative_to(workspace):
            parser.error('Isolated report must stay in the project workspace')
        sources = {name: source.resolve() for name, source in (
            ('HLSDownloaderEngine.exe', args.isolated_engine), ('HLSDownloaderCli.exe', args.isolated_cli))}
        if any(not source.is_file() for source in sources.values()):
            parser.error('An isolated component is missing')
        previous = Path.cwd()
        with tempfile.TemporaryDirectory(prefix='hls-free-parity-', dir=test_tmp()) as temporary:
            directory = Path(temporary)
            args.resources, args.work = directory/'resources', directory/'work'
            args.resources.mkdir()
            for name, source in sources.items():
                shutil.copy2(source, args.resources/name)
            runtime = args.isolated_engine.resolve().parent/'vcruntime140.dll'
            if runtime.is_file():
                shutil.copy2(runtime, args.resources/runtime.name)
            try:
                code = run(args)
            finally:
                os.chdir(previous)
        report = json.loads(args.report.read_text(encoding='utf-8'))
        report['isolated_runtime_cleaned'] = not directory.exists()
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
        return code
    if not args.resources or not args.work:
        parser.error('Sandbox resources/work or both isolated components are required')
    assert os.environ.get('USERNAME') == 'WDAGUtilityAccount', 'Existing resources/work is Windows Sandbox only'
    sys.path.insert(0, r'C:\QADeps')
    args.resources, args.work = args.resources.resolve(), args.work.resolve()
    return run(args)


if __name__ == '__main__':
    raise SystemExit(main())
