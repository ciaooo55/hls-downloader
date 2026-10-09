"""Exercise actual packaged Core HTTP boundaries in an isolated data directory."""
from __future__ import annotations
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
import uuid
from urllib.parse import urlsplit
from msi_checkpoint_fixture import connect, send, wait_task
from smoke_v7_transfer_performance import free_port


def run(args):
    result={'passed':False,'checks':[],'engine_sha256':hashlib.sha256(args.engine.read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='http-boundaries-',dir=Path.cwd()/'.tool-cache/test-tmp') as tmp:
        root=Path(tmp); payload=b'\x00\x00\x00\x18ftypmp42'+bytes(range(256))*2048
        recover=False
        offline=True
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*_): pass
            def do_HEAD(self): self.respond(False)
            def do_GET(self): self.respond(True)
            def do_POST(self): self.respond(True)
            def respond(self,write):
                route=urlsplit(self.path).path
                if route=='/offline' and offline:
                    self.connection.shutdown(2)
                    self.close_connection=True
                    return
                if route=='/redirect':
                    self.send_response(302);self.send_header('Location','/media?signature=qa');self.end_headers();return
                if route=='/media' and ('hlsqa=1' not in self.headers.get('Cookie','') or not self.headers.get('Referer','').endswith('/page')):
                    self.send_error(403);return
                if route=='/post' and (self.command!='POST' or self.rfile.read(int(self.headers.get('Content-Length','0')))!=b'fixture=ok'):
                    self.send_error(400);return
                if route=='/403' and not recover: self.send_error(403);return
                if route=='/404':self.send_error(404);return
                body=b'<!doctype html><html><body>Sign in to continue</body></html>' if route=='/login' else payload*8 if route=='/slow' else payload
                start=0;end=len(body)-1
                ranged=self.headers.get('Range','')
                if ranged.startswith('bytes='):
                    a,b=ranged[6:].split('-',1);start=int(a or 0);end=min(int(b) if b else end,end)
                self.send_response(206 if ranged else 200)
                self.send_header('Content-Type','text/html' if route=='/login' else 'video/mp4')
                self.send_header('Content-Length',str(end-start+1));self.send_header('Accept-Ranges','bytes')
                if ranged:self.send_header('Content-Range',f'bytes {start}-{end}/{len(body)}')
                self.end_headers()
                if write:
                    try:
                        for offset in range(start,end+1,65536):
                            self.wfile.write(body[offset:min(offset+65536,end+1)]);self.wfile.flush()
                            if route=='/slow':time.sleep(.03)
                    except (BrokenPipeError,ConnectionResetError,ConnectionAbortedError):pass
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        threading.Thread(target=server.serve_forever,daemon=True).start()
        port=free_port(); process=None
        def record(name,actual=True):
            result['checks'].append({'name':name,'actual':actual})
            print(json.dumps({'check':name}),flush=True)
        try:
            env={**os.environ,'HLS_V7_DATA_DIR':str(root/'data'),'HLS_V7_DOWNLOAD_DIR':str(root/'downloads'),
                 'HLS_V7_PIPE':rf'\\.\pipe\HLSDownloader.http-{uuid.uuid4().hex}',
                 'HLS_V7_CORE_TCP':'1','HLS_V7_CORE_BIND':f'127.0.0.1:{port}'}
            with (args.report.with_suffix('.core.log')).open('wb') as log:
                process=subprocess.Popen([str(args.engine.resolve())],env=env,stdout=log,stderr=log,creationflags=subprocess.CREATE_NO_WINDOW)
            with connect(port) as ipc:
                r=send(ipc,{'type':'store_settings','request_id':1,'values':{'legal_terms_accepted':True,'legal_terms_version':'2026-08-06-cn-1','download_complete_popup_enabled':False,'download_progress_window_enabled':False,'av_scan_enabled':False}})
                assert r['type']!='error',r
                def download(route,filename='fixture.mp4',**extras):
                    spec={'url':f'http://127.0.0.1:{server.server_port}{route}','resource_kind':'file','filename':filename,'allow_duplicate':True,'concurrency':1,**extras}
                    response=send(ipc,{'type':'command','request_id':2,'command':{'kind':'create_task','spec':spec}})
                    if response.get('type')=='error': return response
                    task=next(e['event']['snapshot'] for e in response['events'] if e['event']['kind']=='task_created')
                    send(ipc,{'type':'command','request_id':3,'command':{'kind':'task_action','task_id':task['task_id'],'action':'start'}})
                    return wait_task(ipc,task['task_id'],lambda t:t['status'] in ('completed','failed'),60)
                def correct(task):
                    path=Path(task['output_path'])
                    assert task['status']=='completed' and path.is_file(),task
                    assert path.stat().st_size==len(payload) and hashlib.sha256(path.read_bytes()).digest()==hashlib.sha256(payload).digest()
                task=download('/redirect',headers={'Cookie':'hlsqa=1','Referer':f'http://127.0.0.1:{server.server_port}/page'})
                correct(task);record('signed-redirect-cookie-referer-complete-sha256')
                body=root/'post-body';body.write_bytes(b'fixture=ok')
                correct(download('/post',request_method='POST',body_path=str(body)))
                record('actual-post-body-download-sha256')
                for status in (403,404):
                    task=download('/'+str(status))
                    assert task['status']=='failed' and task.get('error_message'),task
                    record(f'http-{status}-fails-with-visible-task-error',task['error_message'])
                    if status==403:
                        recover=True
                        send(ipc,{'type':'command','request_id':4,'command':{'kind':'task_action','task_id':task['task_id'],'action':'retry'}})
                        correct(wait_task(ipc,task['task_id'],lambda t:t['status'] in ('completed','failed'),60))
                        record('failed-http-download-retry-recovers-sha256')
                task=download('/offline',filename='network-retry.mp4')
                assert task['status']=='failed' and task.get('error_message'),task
                offline=False
                send(ipc,{'type':'command','request_id':4,'command':{'kind':'task_action','task_id':task['task_id'],'action':'retry'}})
                correct(wait_task(ipc,task['task_id'],lambda t:t['status'] in ('completed','failed'),60))
                record('actual-http-connection-loss-visible-error-and-retry-sha256')
                task=download('/login',filename='login-response.mp4')
                assert task['status']=='failed' and 'HTML' in task.get('error_message',''), task
                assert not task.get('output_path') or not Path(task['output_path']).is_file()
                record('login-html-never-published-as-video')
                task=download('/login',filename='opaque-download',mime_type='video/mp4')
                assert task['status']=='failed' and 'HTML' in task.get('error_message',''), task
                record('browser-media-mime-survives-opaque-filename')
                handoff_id='mime-no-extension-'+uuid.uuid4().hex
                response=send(ipc,{'type':'command','request_id':2,'command':{'kind':'offer_resource','offer':{
                    'url':f'http://127.0.0.1:{server.server_port}/login?handoff={handoff_id}','resource_kind':'file',
                    'filename':'opaque-offer','mime_type':'video/mp4','handoff_id':handoff_id,
                    'source_page_url':f'http://127.0.0.1:{server.server_port}/page'}}})
                assert response.get('type')!='error',response
                response=send(ipc,{'type':'command','request_id':3,'command':{'kind':'accept_handoff',
                    'handoff_id':handoff_id,'filename':'opaque-offer','download_dir':str(root/'downloads'),'trusted_ui':True}})
                assert response.get('type')!='error',response
                task=next((item['event']['snapshot'] for item in response['events'] if item['event']['kind']=='task_created'),None)
                assert task is not None,response
                task=wait_task(ipc,task['task_id'],lambda t:t['status'] in ('completed','failed'),60)
                assert task['status']=='failed' and 'HTML' in task.get('error_message',''),task
                assert not (root/'downloads/opaque-offer').exists()
                record('offer-confirmation-preserves-media-mime-and-rejects-html-without-extension')
                task=download('/login',filename='saved-page.html')
                assert task['status']=='completed' and Path(task['output_path']).read_bytes().startswith(b'<!doctype html')
                record('intentional-html-file-download-still-works')
                blocked=root/'downloads'/'file-as-directory';blocked.write_bytes(b'preserve')
                task=download('/plain',download_dir=str(blocked))
                assert (task.get('type')=='error' and task.get('message')) or (task.get('status')=='failed' and task.get('error_message')),task
                assert blocked.read_bytes()==b'preserve'
                record('invalid-save-directory-visible-error-preserves-existing-file',task.get('message') or task['error_message'])
                response=send(ipc,{'type':'command','request_id':5,'command':{'kind':'create_task','spec':{
                    'url':f'http://127.0.0.1:{server.server_port}/slow','resource_kind':'file','filename':'restart-resume.bin','concurrency':1}}})
                assert response.get('type')!='error',response
                record('invalid-directory-task-does-not-block-following-download')
                task=next(e['event']['snapshot'] for e in response['events'] if e['event']['kind']=='task_created')
                def action(stream,value):
                    return send(stream,{'type':'command','request_id':6,'command':{'kind':'task_action','task_id':task['task_id'],'action':value}})
                action(ipc,'start')
                wait_task(ipc,task['task_id'],lambda t:t['downloaded_bytes']>=262144,70)
                action(ipc,'pause')
                paused=wait_task(ipc,task['task_id'],lambda t:t['status']=='paused' and t['active_workers']==0,71)
                assert 0<paused['downloaded_bytes']<len(payload)*8
                process.terminate();process.wait(timeout=10)
                with args.report.with_suffix('.restart.log').open('wb') as log:
                    process=subprocess.Popen([str(args.engine.resolve())],env=env,stdout=log,stderr=log,creationflags=subprocess.CREATE_NO_WINDOW)
                with connect(port) as restored:
                    recovered=wait_task(restored,task['task_id'],lambda t:t['status']=='paused',72)
                    assert recovered['downloaded_bytes']>=paused['downloaded_bytes']
                    action(restored,'resume')
                    done=wait_task(restored,task['task_id'],lambda t:t['status'] in ('completed','failed'),73)
                    assert done['status']=='completed',done
                    output=Path(done['output_path'])
                    assert output.stat().st_size==len(payload)*8 and hashlib.sha256(output.read_bytes()).digest()==hashlib.sha256(payload*8).digest()
                record('actual-core-restart-preserves-paused-task-and-resumes-sha256')
                result['passed']=True
        except Exception as error:
            result['error']=str(error);result['traceback']=traceback.format_exc()
        finally:
            if process:
                process.terminate()
                with contextlib.suppress(subprocess.TimeoutExpired):process.wait(timeout=10)
            server.shutdown();server.server_close()
            args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf8')
    return 0 if result['passed'] else 1


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--engine',type=Path,required=True);p.add_argument('--report',type=Path,required=True)
    raise SystemExit(run(p.parse_args()))
