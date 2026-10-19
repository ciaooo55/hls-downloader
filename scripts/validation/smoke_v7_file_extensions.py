"""Guest-only acceptance for response filenames and actual downloaded formats."""
from pathlib import Path
import argparse,hashlib,io,json,os,subprocess,threading,traceback,uuid,zipfile
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from smoke_v7_portable_app import Pipe,wait

def run(runtime,report_path):
    assert os.environ.get('HLS_VM_ACCEPTANCE')=='1','Run only in the acceptance VM'
    report_path.parent.mkdir(parents=True,exist_ok=True)
    fixture=report_path.parent/(report_path.stem+'-media');fixture.mkdir(exist_ok=True)
    report={'passed':False,'checks':[],'engine_sha256':hashlib.sha256((runtime/'app/resources/HLSDownloaderEngine.exe').read_bytes()).hexdigest()}
    created=[];server=None;engine_process=None;engine_log=None
    pipe_name=rf'\\.\pipe\HLSSuffix-{uuid.uuid4().hex}'
    def request(value):
        pipe=Pipe(pipe_name)
        try:return pipe.request(value)
        finally:pipe.close()
    def command(value):return request({'type':'command','request_id':1,'command':value})
    def task(tid):return next(t for t in request({'type':'snapshot','request_id':2})['tasks'] if t['task_id']==tid)
    try:
        root=Path(r'C:\QAWork')/('suffix-'+uuid.uuid4().hex)
        root.mkdir(parents=True)
        environment={**os.environ,'HLS_V7_PIPE':pipe_name,'HLS_V7_DATA_DIR':str(root/'data'),
                     'HLS_V7_DOWNLOAD_DIR':str(root/'downloads')}
        engine_log=report_path.with_suffix('.engine.log').open('wb')
        engine_process=subprocess.Popen([str(runtime/'app/resources/HLSDownloaderEngine.exe')],
            env=environment,stdout=engine_log,stderr=engine_log,creationflags=subprocess.CREATE_NO_WINDOW)
        def ready():
            try:return request({'type':'hello','protocol':'hls-downloader-v7-core','version':1}).get('type')=='hello'
            except OSError:return None
        wait(ready,'Suffix acceptance Core did not start',15)
        settings=request({'type':'store_settings','request_id':0,'values':{
            'download_progress_window_enabled':False,'download_complete_popup_enabled':False,'av_scan_enabled':False}})
        assert settings.get('type')!='error',settings
        ffmpeg=runtime/'app/resources/ffmpeg.exe'
        for ext,args in [('mp4',['-c:v','libx264']),('webm',['-c:v','libvpx-vp9']),('mkv',['-c:v','libx264']),('mov',['-c:v','libx264'])]:
            subprocess.run([str(ffmpeg),'-y','-v','error','-f','lavfi','-i','testsrc2=size=160x90:rate=10','-t','1',*args,str(fixture/('video.'+ext))],check=True,timeout=45)
        for ext,args in [('mp3',['-c:a','libmp3lame']),('wav',['-c:a','pcm_s16le']),('aac',['-c:a','aac']),('m4a',['-c:a','aac']),('flac',['-c:a','flac']),('ogg',['-c:a','libvorbis'])]:
            subprocess.run([str(ffmpeg),'-y','-v','error','-f','lavfi','-i','sine=frequency=440','-t','1',*args,str(fixture/('audio.'+ext))],check=True,timeout=45)
        from PIL import Image
        for ext in ('png','jpg','gif','webp','pdf'):
            Image.new('RGB',(32,32),(25,90,180)).save(fixture/('image.'+ext))
        archive=io.BytesIO()
        with zipfile.ZipFile(archive,'w') as output:output.writestr('hello.txt','download extension acceptance')
        def office_archive(folder,part,content):
            buffer=io.BytesIO()
            with zipfile.ZipFile(buffer,'w') as output:
                output.writestr('[Content_Types].xml',f'<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/{folder}/{part}" ContentType="application/vnd.openxmlformats-officedocument.{content}+xml"/></Types>')
                output.writestr('_rels/.rels',f'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="{folder}/{part}"/></Relationships>')
                output.writestr(folder+'/'+part,'<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Acceptance</w:t></w:r></w:p></w:body></w:document>' if folder=='word' else '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheets/></workbook>')
            return buffer.getvalue()
        cases=[]
        for path in sorted(fixture.iterdir()):
            if path.is_file():cases.append((path.read_bytes(),'application/octet-stream','',path.suffix))
        cases.extend([
            (archive.getvalue(),'application/octet-stream','','.zip'),
            (office_archive('word','document.xml','wordprocessingml.document.main'),'application/vnd.openxmlformats-officedocument.wordprocessingml.document','attachment; filename="report.docx"','.docx'),
            (office_archive('xl','workbook.xml','spreadsheetml.sheet.main'),'application/octet-stream',"attachment; filename=wrong.zip; filename*=UTF-8''%E6%8A%A5%E5%91%8A%2Exlsx",'.xlsx'),
            (b'{"hello":true}','application/json; charset=utf-8','','.json'),
            (b'a,b\n1,2\n','text/csv','attachment; filename="../report.csv"','.csv'),
            (b'plain response','application/octet-stream','attachment; filename="report.txt"','.txt'),
            ((fixture/'video.mp4').read_bytes(),'image/png','attachment; filename=wrong.jpg','.mp4'),
        ])
        direct_count=len(cases)
        cases.extend([cases[-5],cases[-4],cases[-2]])
        class Handler(BaseHTTPRequestHandler):
            def log_message(self,*_):pass
            def do_GET(self):
                if self.path.startswith('/redirect/'):
                    self.send_response(302);self.send_header('Location',self.path.removeprefix('/redirect'));self.send_header('Content-Length','0');self.end_headers();return
                body,mime,disposition,_=cases[int(self.path.split('/')[1].split('.')[0])]
                offset=int(self.headers.get('Range','bytes=0-').split('=')[1].split('-')[0])
                end=self.headers.get('Range','').split('-')[-1]
                limit=min(len(body),int(end)+1) if end.isdigit() else len(body)
                self.send_response(206 if self.headers.get('Range') else 200)
                self.send_header('Content-Type',mime);self.send_header('Content-Length',str(limit-offset))
                if disposition:self.send_header('Content-Disposition',disposition)
                if self.headers.get('Range'):self.send_header('Content-Range',f'bytes {offset}-{limit-1}/{len(body)}')
                self.send_header('Accept-Ranges','bytes');self.end_headers()
                self.wfile.write(body[offset:limit])
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        threading.Thread(target=server.serve_forever,daemon=True).start()
        for index,(body,mime,disposition,extension) in enumerate(cases):
            name=f'extension-{uuid.uuid4().hex}.php'
            transport='direct' if index<direct_count else 'system'
            response=command({'kind':'create_task','spec':{'url':f'http://127.0.0.1:{server.server_port}/redirect/{index}.php?download=1&name=wrong.html','filename':name,'resource_kind':'file','concurrency':2,'proxy':f'hls-downloader://{transport}-proxy'}})
            tid=next(e['event']['snapshot']['task_id'] for e in response['events'] if e['event']['kind']=='task_created');created.append(tid)
            current=wait(lambda:task(tid) if task(tid)['status'] in ('completed','failed') else None,'File did not finish',45)
            assert current['status']=='completed',current
            path=Path(current['output_path'])
            assert path.suffix==extension,(path,extension)
            assert current['filename']==path.name,(current['filename'],path.name)
            assert hashlib.sha256(path.read_bytes()).digest()==hashlib.sha256(body).digest(),'File content changed'
            report['checks'].append({'expected_extension':extension,'filename':path.name,'response_mime':mime,'transport':transport,'hash_preserved':True})
            print(extension+' actual download, suffix, task filename and content hash passed',flush=True)
        report['passed']=True
    except Exception:report['error']=traceback.format_exc();print(report['error'],flush=True)
    finally:
        for tid in created:
            try:command({'kind':'task_action','task_id':tid,'action':'delete_files'})
            except Exception:pass
        if server:server.shutdown();server.server_close()
        if engine_process:
            engine_process.terminate()
            engine_process.wait(timeout=10)
        if engine_log:engine_log.close()
        report_path.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    return 0 if report['passed'] else 1

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--runtime',type=Path,required=True);parser.add_argument('--report',type=Path,required=True)
    args=parser.parse_args();raise SystemExit(run(args.runtime,args.report))
