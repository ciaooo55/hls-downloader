"""Sandbox-only, three-round off/on comparison of the experimental HTTP scheduler.

Local fixtures and Core-only resource samples; this is not a whole-product
IDM/AB performance comparison or a general speed claim.
"""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import statistics
import subprocess
import threading
import time
import traceback

from v7_core_ipc import connect, send_frame as send


def main():
    args=argparse.ArgumentParser()
    args.add_argument('--engine',type=Path,required=True)
    args.add_argument('--work',type=Path,required=True)
    args.add_argument('--report',type=Path,required=True)
    args=args.parse_args()
    assert os.environ.get('USERNAME')=='WDAGUtilityAccount','Windows Sandbox only'
    import psutil
    args.work.mkdir(parents=True,exist_ok=True)
    block=bytes(range(256))*128
    total=32*1024*1024
    expected=hashlib.sha256(block*(total//len(block))).hexdigest()
    result={'passed':False,'finished':False,'scope':'Core-only experimental off/on local comparison',
            'engine_sha256':hashlib.sha256(args.engine.read_bytes()).hexdigest(),'samples':[]}
    current={'mode':'normal','requests':[]}
    class Server(ThreadingHTTPServer):
        daemon_threads=True
        def handle_error(self,*_): pass
    class Handler(BaseHTTPRequestHandler):
        protocol_version='HTTP/1.1'
        def log_message(self,*_):pass
        def do_HEAD(self):self.respond(False)
        def do_GET(self):self.respond(True)
        def respond(self,body):
            first,last=0,total-1
            value=self.headers.get('Range')
            if value:
                a,b=value[6:].split('-');first=int(a);last=min(int(b) if b else last,last)
            record={'start':first,'end':last,'bytes_written':0}
            current['requests'].append(record)
            self.send_response(206 if value else 200)
            self.send_header('Content-Length',str(last-first+1));self.send_header('Accept-Ranges','bytes');self.send_header('ETag','"split-fixture"')
            if value:self.send_header('Content-Range',f'bytes {first}-{last}/{total}')
            self.end_headers()
            if not body:return
            for at in range(first,last+1,len(block)):
                size=min(len(block),last-at+1);pos=at%len(block);data=(block[pos:]+block[:pos])[:size]
                try:self.wfile.write(data);self.wfile.flush();record['bytes_written']+=len(data)
                except OSError:return
                if current['mode']=='throttled':time.sleep(.002)
                elif current['mode']=='tail' and first==0 and last>0:time.sleep(.02)
    server=Server(('127.0.0.1',18674),Handler)
    threading.Thread(target=server.serve_forever,daemon=True).start()
    def save():args.report.write_text(json.dumps(result,indent=2),encoding='utf-8')
    try:
        for round_number in range(3):
            for mode in ['normal','throttled','tail']:
                for enabled in ([False,True] if round_number%2==0 else [True,False]):
                    name=f'{round_number}-{mode}-{int(enabled)}';root=args.work/name;root.mkdir()
                    current.update(mode=mode,requests=[])
                    env=dict(os.environ,HLS_VM_ACCEPTANCE='1',HLS_V7_DATA_DIR=str(root/'data'),
                        HLS_V7_DOWNLOAD_DIR=str(root/'downloads'),HLS_V7_CORE_TCP='1',HLS_V7_CORE_BIND='127.0.0.1:18678',
                        HLS_V7_PIPE=rf'\\.\pipe\HLSSplit-{name}',HLS_HTTP_ACTIVE_SPLIT='1' if enabled else '0')
                    with (root/'core.log').open('wb') as log:
                        cold=time.monotonic();proc=subprocess.Popen([str(args.engine)],env=env,stdout=log,stderr=log,creationflags=subprocess.CREATE_NO_WINDOW)
                        process=psutil.Process(proc.pid)
                        try:
                            with connect(18678) as ipc:
                                cold=time.monotonic()-cold
                                send(ipc,{'type':'store_settings','request_id':1,'values':{'download_progress_window_enabled':False,'download_complete_popup_enabled':False,'av_scan_enabled':False}})
                                before=process.cpu_times();started=time.monotonic();peak=0
                                response=send(ipc,{'type':'command','request_id':2,'command':{'kind':'create_task','spec':{'url':'http://127.0.0.1:18674/fixture.bin','filename':'fixture.bin','resource_kind':'file','concurrency':4}}})
                                task_id=next(e['event']['snapshot']['task_id'] for e in response['events'] if e['event']['kind']=='task_created')
                                while True:
                                    task=next(t for t in send(ipc,{'type':'snapshot','request_id':3})['tasks'] if t['task_id']==task_id)
                                    peak=max(peak,process.memory_info().private)
                                    if task['status'] in ['completed','failed']:break
                                    assert time.monotonic()-started<30,task
                                    time.sleep(.025)
                                elapsed=time.monotonic()-started;after=process.cpu_times()
                                assert task['status']=='completed',task
                                digest=hashlib.sha256(Path(task['output_path']).read_bytes()).hexdigest();assert digest==expected
                                sample={'round':round_number,'mode':mode,'active_split':enabled,'connections':4,'cold_core_seconds':cold,
                                    'download_seconds':elapsed,'cpu_seconds':after.user+after.system-before.user-before.system,'peak_private_bytes':peak,
                                    'sha256':digest,'requests':list(current['requests'])}
                                result['samples'].append(sample);save()
                        finally:
                            if proc.poll() is None:proc.kill();proc.wait(timeout=10)
        result['medians']={mode:{str(enabled):statistics.median(s['download_seconds'] for s in result['samples'] if s['mode']==mode and s['active_split']==enabled) for enabled in [False,True]} for mode in ['normal','throttled','tail']}
        result['passed']=len(result['samples'])==18
        result['default_enabled']=False
        result['default_decision']='Keep opt-in pending full interrupted-transfer and whole-product regression matrix.'
    except Exception as e:result['error']=repr(e);result['traceback']=traceback.format_exc()
    finally:
        server.shutdown();server.server_close();result['finished']=True;save()
    return 0 if result['passed'] else 1

if __name__=='__main__':raise SystemExit(main())
