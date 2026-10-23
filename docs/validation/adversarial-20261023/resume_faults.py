"""Fault injection against the candidate Core, in Sandbox only."""
import hashlib, json, os, socket, subprocess, threading, time, traceback, uuid
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from msi_checkpoint_fixture import connect, send, wait_task

assert os.environ.get('HLS_VM_ACCEPTANCE') == '1'
root = Path(r'C:\QAWork') / ('resume-faults-' + os.environ.get('HLS_QA_ROUND', '1')); root.mkdir(parents=True, exist_ok=True)
report_path = Path(r'C:\QAReports\reports') / ('resume-faults-' + os.environ.get('HLS_QA_ROUND', '1') + '.json')
body = bytes((index % 251) + 1 for index in range(1024 * 1024))
requests = []
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def do_GET(self):
        value = self.headers.get('Range', '')
        first, last, status = 0, len(body)-1, 200
        if value and not self.path.startswith('/no-range'):
            parts = value[6:].split('-'); first = int(parts[0]); last = min(int(parts[1]) if parts[1] else last, last); status=206
        requests.append({'path':self.path,'range':value,'bytes':last-first+1,'if_range':self.headers.get('If-Range')})
        self.send_response(status); self.send_header('Content-Length',str(last-first+1)); self.send_header('Content-Type','application/octet-stream')
        self.send_header('ETag','"resume-fixture"'); self.send_header('Accept-Ranges','bytes')
        if status==206: self.send_header('Content-Range',f'bytes {first}-{last}/{len(body)}')
        self.end_headers()
        try: self.wfile.write(body[first:last+1])
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError): pass
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
threading.Thread(target=server.serve_forever,daemon=True).start()
with socket.socket() as probe: probe.bind(('127.0.0.1',0)); port=probe.getsockname()[1]
env={**os.environ,'HLS_V7_DATA_DIR':str(root/'data'),'HLS_V7_DOWNLOAD_DIR':str(root/'downloads'),
     'HLS_V7_PIPE':rf'\\.\pipe\hls-resume-{uuid.uuid4().hex}','HLS_V7_CORE_TCP':'1','HLS_V7_CORE_BIND':f'127.0.0.1:{port}'}
engine=Path(r'C:\QAWork\runtime\app\resources\HLSDownloaderEngine.exe')
report={'passed':False,'engine_sha256':hashlib.sha256(engine.read_bytes()).hexdigest(),'checks':[]}
def save(): report_path.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
with (root/'core.log').open('wb') as log:
    process=subprocess.Popen([str(engine)],env=env,stdout=log,stderr=log,creationflags=subprocess.CREATE_NO_WINDOW)
try:
    with connect(port) as ipc:
        result=send(ipc,{'type':'store_settings','request_id':1,'values':{'legal_terms_accepted':True,'legal_terms_version':'2026-08-06-cn-1','download_complete_popup_enabled':False,'download_progress_window_enabled':False,'av_scan_enabled':False,'auto_retry_failed_max':0}})
        assert result['type']!='error',result
        def case(name, mode):
            record={'name':name,'passed':False}; report['checks'].append(record); save()
            try:
                # CreateTask 自动调度；先关闭隔离队列，避免与断点夹具写入竞争。
                def queue_enabled(enabled):
                    reply=send(ipc,{'type':'store_settings','request_id':20,'values':{'queue_profiles':[{'id':'default','name':'Fault injection','enabled':enabled}]}})
                    assert reply['type']!='error',reply
                queue_enabled(False)
                url=f'http://127.0.0.1:{server.server_port}/'+('no-range' if mode=='no-range' else mode)
                spec={'url':url,'filename':mode+'.bin','resource_kind':'file','concurrency':1,'expected_size':len(body),'allow_duplicate':True}
                if mode!='missing-validator': spec['etag']='"resume-fixture"'
                result=send(ipc,{'type':'command','request_id':2,'command':{'kind':'create_task','spec':spec}})
                assert result['type']!='error',result
                task=next(e['event']['snapshot'] for e in result['events'] if e['event']['kind']=='task_created')
                task_id=task['task_id']; task_root=root/'downloads'/'.hls-tasks'/task_id; task_root.mkdir(parents=True,exist_ok=True)
                prefix=256*1024
                output=task_root/'payload.downloading'
                output.write_bytes(body[:prefix])
                checkpoint={'version':2,'resource_key':url,'total':len(body),'etag':'"resume-fixture"','last_modified':'','ranges':[[0,prefix-1]]}
                if mode=='truncated': checkpoint['ranges']=[[0,len(body)-1]]
                if mode=='no-range': checkpoint['resource_key']='stale-other-resource'
                (task_root/'native-engine.ranges.json').write_text(json.dumps(checkpoint),encoding='utf-8')
                start=len(requests)
                assert not any(r['path']=='/'+mode for r in requests),'fixture ran before checkpoint was ready'
                queue_enabled(True)
                result=send(ipc,{'type':'command','request_id':3,'command':{'kind':'task_action','task_id':task_id,'action':'start'}})
                assert result['type']!='error',result
                done=wait_task(ipc,task_id,lambda t:t['status'] in ('completed','failed'),30)
                record['status']=done['status']; record['error']=done.get('error_message'); record['requests']=requests[start:]
                record['downloaded_bytes']=done['downloaded_bytes']
                final=Path(done.get('output_path') or root/'absent')
                record['expected_sha256']=hashlib.sha256(body).hexdigest()
                record['actual_sha256']=hashlib.sha256(final.read_bytes()).hexdigest() if final.is_file() else None
                assert done['status']=='completed',done.get('error_message')
                assert record['actual_sha256']==record['expected_sha256'],'task completed with corrupted bytes after truncation'
                if mode in ('valid','missing-validator'):
                    transfers=[r for r in record['requests'] if r['bytes']>1]
                    assert transfers and all(int(r['range'][6:].split('-')[0])>=prefix for r in transfers),'downloaded already checkpointed prefix again'
                record['passed']=True
            except Exception as error:
                record['failure']=str(error); record['traceback']=traceback.format_exc()
            save()
        for name,mode in [('valid checkpoint skips durable prefix','valid'),('truncated local file never publishes corrupt completion','truncated'),('known size without supplied validator reuses verified prefix','missing-validator'),('invalid checkpoint can restart when server ignores Range','no-range')]: case(name,mode)
        # A full bounded task-log cycle: inject progress through the public command interface.
        result=send(ipc,{'type':'command','request_id':10,'command':{'kind':'create_task','spec':{'url':f'http://127.0.0.1:{server.server_port}/logging','filename':'log.bin','resource_kind':'file','scheduled_start_at':'2099-01-01T00:00:00Z'}}})
        task_id=next(e['event']['snapshot']['task_id'] for e in result['events'] if e['event']['kind']=='task_created')
        for index in range(520):
            send(ipc,{'type':'command','request_id':11,'command':{'kind':'update_progress','task_id':task_id,'downloaded_bytes':index,'total_bytes':1000,'speed_bytes_per_sec':1,'stage':f'transfer-{index}','status':'queued'}})
        result=send(ipc,{'type':'command','request_id':12,'command':{'kind':'get_task_log','task_id':task_id}})
        lines=next(e['event']['lines'] for e in result['events'] if e['event']['kind']=='task_log')
        import sqlite3
        with sqlite3.connect(root/'data'/'data.db') as db:
            stored = db.execute('SELECT COUNT(*) FROM logs WHERE task_id=?',(task_id,)).fetchone()[0]
        report['checks'].append({'name':'task log prunes 520 unique entries to 500 stored and returned records','passed':len(lines)==500 and stored==500,'count':len(lines),'stored_count':stored})
        report['passed']=all(item['passed'] for item in report['checks']); save()
except Exception as error:
    report['error']=str(error); report['traceback']=traceback.format_exc();save()
finally:
    process.terminate();process.wait(timeout=10);server.shutdown()
raise SystemExit(0 if report['passed'] else 1)
