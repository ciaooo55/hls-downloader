"""Guest-only regression for progress and pause/resume during separate audio downloads."""
from pathlib import Path
import argparse,hashlib,json,os,subprocess,threading,time,traceback,uuid
from http.server import SimpleHTTPRequestHandler,ThreadingHTTPServer
from smoke_v7_portable_app import Pipe,wait

def run(runtime,report_path):
    assert os.environ.get('HLS_VM_ACCEPTANCE')=='1','Run only in the acceptance VM'
    report_path.parent.mkdir(parents=True,exist_ok=True)
    fixture=report_path.parent/(report_path.stem+'-media');fixture.mkdir(exist_ok=True)
    report={'passed':False,'checks':[], 'components':{relative:hashlib.sha256((runtime/relative).read_bytes()).hexdigest() for relative in ('app/resources/HLSDownloaderEngine.exe','app/resources/HLSDownloaderNativeHost.exe')}};created=[];server=None
    def save():report_path.write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    def core(value):
        p=Pipe()
        try:return p.request(value)
        finally:p.close()
    def command(value):return core({'type':'command','request_id':1,'command':value})
    def task(tid):return next(t for t in core({'type':'snapshot','request_id':2})['tasks'] if t['task_id']==tid)
    def action(tid,name):return command({'kind':'task_action','task_id':tid,'action':name})
    def ffmpeg(args):subprocess.run([str(runtime/'app/resources/ffmpeg.exe'),'-y','-v','error',*args],cwd=fixture,check=True,timeout=60)
    gates=[]
    try:
        ffmpeg(['-f','lavfi','-i','testsrc2=size=320x180:rate=25','-t','8','-c:v','libx264','-g','25','-an','-hls_time','1','-hls_list_size','0','-hls_segment_filename',str(fixture/'video-%d.ts'),str(fixture/'video.m3u8')])
        ffmpeg(['-f','lavfi','-i','sine=frequency=440:sample_rate=48000','-t','8','-c:a','aac','-vn','-hls_time','1','-hls_list_size','0','-hls_segment_filename',str(fixture/'audio-%d.ts'),str(fixture/'audio.m3u8')])
        (fixture/'master.m3u8').write_text('#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",NAME="main",DEFAULT=YES,AUTOSELECT=YES,URI="audio.m3u8"\n#EXT-X-STREAM-INF:BANDWIDTH=500000,AUDIO="a"\nvideo.m3u8\n',encoding='utf-8')
        ffmpeg(['-f','lavfi','-i','testsrc2=size=320x180:rate=25','-f','lavfi','-i','sine=frequency=660:sample_rate=48000','-t','8','-map','0:v','-map','1:a','-c:v','libx264','-g','25','-c:a','aac','-seg_duration','1','-f','dash',str(fixture/'dash.mpd')])
        gate=threading.Event();requested=threading.Event();gates.append(gate)
        class Handler(SimpleHTTPRequestHandler):
            def __init__(self,*args,**kwargs):super().__init__(*args,directory=str(fixture),**kwargs)
            def log_message(self,*_):pass
            def send_error(self,code,message=None,explain=None):
                if code==404:
                    report.setdefault('missing_urls',[]).append(self.path)
                    print('Missing fixture URL: '+self.path,flush=True)
                super().send_error(code,message,explain)
            def do_GET(self):
                name=self.path.split('?')[0].rsplit('/',1)[-1]
                if name.startswith('audio-') or name.startswith('chunk-stream1-') or name.startswith('init-stream1'):
                    requested.set();gate.wait(40);time.sleep(.8)
                elif name.startswith('video-') or name.startswith('chunk-stream0-'):time.sleep(.25)
                super().do_GET()
        server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        threading.Thread(target=server.serve_forever,daemon=True).start()
        for manifest,kind in [('master.m3u8','hls'),('dash.mpd','dash')]:
            gate.clear();requested.clear()
            response=command({'kind':'create_task','spec':{'url':f'http://127.0.0.1:{server.server_port}/{manifest}','filename':f'tracks-{kind}-{uuid.uuid4().hex}.mkv','resource_kind':kind,'concurrency':1}})
            tid=next(e['event']['snapshot']['task_id'] for e in response['events'] if e['event']['kind']=='task_created');created.append(tid)
            wait(lambda:requested.is_set() or task(tid)['status']=='failed','Selected audio never requested',45)
            assert task(tid)['status']!='failed',task(tid).get('error_message')
            time.sleep(3.5)
            video_done=task(tid)
            assert video_done['status']=='downloading',video_done
            assert 0<video_done['completed_ranges']<video_done['total_ranges'],'Video completion incorrectly reports whole download at 100%'
            gate.set();samples=[];paused=False;deadline=time.monotonic()+90
            while time.monotonic()<deadline:
                current=task(tid)
                samples.append({k:current[k] for k in ('status','downloaded_bytes','speed_bytes_per_sec','completed_ranges','total_ranges')})
                if current['status']=='failed':raise AssertionError(current['error_message'])
                if not paused and current['completed_ranges']>=video_done['completed_ranges']+2 and current['status']=='downloading':
                    action(tid,'pause')
                    def checkpointed():
                        snapshot=task(tid)
                        return snapshot if snapshot['status']=='paused' and snapshot['log_tail'][-1].startswith('paused waiting') else None
                    settled=wait(checkpointed,'Audio worker did not finish saving its pause checkpoint',15)
                    time.sleep(1)
                    assert task(tid)['status']=='paused','Pause incorrectly published a truncated success'
                    assert task(tid)['downloaded_bytes']==settled['downloaded_bytes'],'Paused task kept advancing'
                    action(tid,'resume');paused=True
                if current['status']=='completed':break
                time.sleep(.2)
            assert current['status']=='completed' and paused,current
            running=[s for s in samples if s['status']=='downloading']
            assert all(a['downloaded_bytes']<=b['downloaded_bytes'] for a,b in zip(running,running[1:])),'Resumed media progress moved backwards'
            audio=[s for s in samples if s['status']=='downloading' and s['downloaded_bytes']>video_done['downloaded_bytes']]
            assert any(s['speed_bytes_per_sec']>0 for s in audio),'Selected audio bytes do not update speed'
            assert current['completed_ranges']==current['total_ranges']>video_done['completed_ranges'],current
            output=Path(current['output_path'])
            assert output.suffix=='.mp4' and current['filename']==output.name,'Merged MP4 kept an incorrect requested suffix'
            metadata=json.loads(subprocess.check_output([str(runtime/'app/resources/ffprobe.exe'),'-v','error','-show_streams','-show_format','-of','json',str(output)],timeout=30))
            assert {'audio','video'} <= {s['codec_type'] for s in metadata['streams']},metadata
            assert float(metadata['format']['duration'])>=7.8,metadata['format']
            subprocess.run([str(runtime/'app/resources/ffmpeg.exe'),'-v','error','-i',str(output),'-f','null','NUL'],check=True,timeout=45)
            report['checks'].append({'kind':kind,'video_finished':{k:video_done[k] for k in ('downloaded_bytes','completed_ranges','total_ranges')},'completed_segments':current['completed_ranges'],'audio_pause_resume':True,'decoded_audio_video':True,'samples':samples});save()
            print(kind+' separate audio progress, rate, pause/resume and decode passed',flush=True)
        report['passed']=True
    except Exception:
        report['error']=traceback.format_exc();print(report['error'],flush=True)
        report['failed_tasks']=[task(tid) for tid in created]
    finally:
        for gate in gates:gate.set()
        for tid in created:
            try:action(tid,'delete_files')
            except Exception:pass
        if server:server.shutdown();server.server_close()
        save()
    return 0 if report['passed'] else 1

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--runtime',type=Path,required=True);parser.add_argument('--report',type=Path,required=True)
    args=parser.parse_args();raise SystemExit(run(args.runtime,args.report))
