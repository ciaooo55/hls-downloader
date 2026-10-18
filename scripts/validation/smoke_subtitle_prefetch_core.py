"""Windows 沙盒：实际 Core/FFmpeg 窗口解码与短音频云端提交验收。"""
import argparse
import base64
import concurrent.futures
import contextlib
from functools import partial
from http.server import ThreadingHTTPServer
import json
import math
import os
import shutil
import statistics
from pathlib import Path
import socket
import struct
import subprocess
import threading
import time
import traceback
import uuid
import psutil

from msi_checkpoint_fixture import connect, send
from smoke_extension_subtitles import QuietHandler


def wait(predicate, description, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.1)
    raise AssertionError(description)


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Run only in the acceptance VM'
    root = Path(os.environ['TEMP']) / ('hls-prefetch-' + uuid.uuid4().hex)
    root.mkdir()
    fixture = root / 'media'
    fixture.mkdir()
    process = server = None
    report = {'passed': False, 'checks': []}
    key = args.key_file.read_text(encoding='utf-8-sig').strip()
    args.key_file.unlink()
    ffmpeg = args.tools / 'ffmpeg.exe'
    try:
        def ff(*argv):
            result = subprocess.run([str(ffmpeg), '-nostdin', '-y', '-v', 'error', *map(str, argv)],
                                    capture_output=True, timeout=60, cwd=fixture)
            assert result.returncode == 0, result.stderr.decode(errors='replace')
            return result.stdout

        ff('-f', 'lavfi', '-i', 'testsrc2=size=160x90:rate=10', '-f', 'lavfi', '-i',
           'aevalsrc=0.2*sin(2*PI*(440+220*floor(t/4))*t):s=48000',
           '-t', '16', '-c:v', 'libx264', '-g', '20', '-c:a', 'aac', fixture / 'sample.mp4')
        ff('-i', fixture / 'sample.mp4', '-vn', '-c:a', 'copy', fixture / 'sample.m4a')
        ff('-i', fixture / 'sample.mp4', '-c', 'copy', '-hls_time', '2', '-hls_list_size', '0', fixture / 'sample.m3u8')
        ff('-i', fixture / 'sample.mp4', '-c', 'copy', '-f', 'dash', '-seg_duration', '2', fixture / 'sample.mpd')
        stall = threading.Event()

        class Handler(QuietHandler):
            def do_GET(self):
                if self.path == '/stall':
                    stall.set()
                    time.sleep(4)
                else:
                    super().do_GET()

        server = ThreadingHTTPServer(('127.0.0.1', 0), partial(Handler, directory=str(fixture)))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        with socket.socket() as candidate:
            candidate.bind(('127.0.0.1', 0))
            port = candidate.getsockname()[1]
        env = {**os.environ, 'HLS_V7_DATA_DIR': str(root / 'data'), 'HLS_V7_DOWNLOAD_DIR': str(root / 'downloads'),
               'HLS_V7_PIPE': rf'\\.\pipe\HLSSubtitlePrefetch-{uuid.uuid4().hex}',
               'HLS_V7_CORE_TCP': '1', 'HLS_V7_CORE_BIND': f'127.0.0.1:{port}', 'HLS_FFMPEG': str(ffmpeg)}
        with args.report.with_suffix('.core.log').open('wb') as log:
            process = subprocess.Popen([str(args.engine)], env=env, stdout=log, stderr=log,
                                       creationflags=subprocess.CREATE_NO_WINDOW)
        with connect(port) as ipc:
            ipc.settimeout(20)
            def call(action, sid='', **values):
                response = send(ipc, {'type': 'subtitle', 'request_id': 1,
                                     'request': {'action': action, 'session_id': sid, 'owner': 'sandbox-prefetch', **values}})
                assert response.get('result', {}).get('ok'), response
                return response['result']

            call('configure', provider='qwencloud', api_key=key)
            origin = f'http://127.0.0.1:{server.server_port}'
            def start(sid, url):
                call('start', sid, source='en', target='zh', epoch=0, audio_source={'url': url})

            def poll(sid):
                result = call('poll', sid, after_sequence=0)
                assert result['status'] != 'failed', result
                return result

            if args.stage == 'all':
                for suffix in ('m4a', 'mp4', 'm3u8', 'mpd'):
                    sid = 'decode-' + suffix
                    start(sid, f'{origin}/sample.{suffix}')
                    for position in (1.25, 5.5, 12.0):
                        prepared = call('prepare_audio', sid, epoch=0, position=position, duration=2.5)
                        actual = base64.b64decode(prepared['audio'])
                        assert abs(len(actual) / 32000 - 2.5) < .005, (suffix, prepared['duration'])
                        expected = ff('-ss', position, '-i', fixture / 'sample.m4a', '-t', '2.5',
                                      '-ac', '1', '-ar', '16000', '-f', 's16le', 'pipe:1')
                        a = struct.unpack(f'<{len(actual)//2}h', actual)
                        b = struct.unpack(f'<{len(expected)//2}h', expected)
                        # AAC 的容器起始时间会改变载波相位；检查每半秒的频率和能量，不能逐样本比较。
                        metrics = []
                        for offset in range(0, min(len(a), len(b))-7999, 8000):
                            aa, bb = a[offset:offset+8000], b[offset:offset+8000]
                            def period(values):
                                indices = [i for i, (x, y) in enumerate(zip(values, values[1:])) if x < 0 <= y]
                                return statistics.median(b-a for a,b in zip(indices,indices[1:]))
                            rms = lambda values: math.sqrt(sum(x*x for x in values)/len(values))
                            metrics.append([period(aa), period(bb), round(rms(aa)/rms(bb), 3)])
                        assert all(abs(x-y) <= .5 and .8 < ratio < 1.1 for x,y,ratio in metrics), (suffix, position, metrics)
                        report.setdefault('decode_metrics', []).append({'format':suffix, 'position':position, 'half_second_period_and_energy':metrics})
                    call('stop', sid)
                    wait(lambda: poll(sid)['status'] == 'stopped', 'decode session did not stop')
                    report['checks'].append(f'{suffix}: three nonzero seek windows match reference audio duration, frequency and energy')

                start('cancel', origin + '/stall')
                with concurrent.futures.ThreadPoolExecutor() as pool:
                    def prepare():
                        with connect(port) as other:
                            return send(other, {'type': 'subtitle', 'request_id': 2, 'request': {
                                'action':'prepare_audio', 'owner':'sandbox-prefetch', 'session_id':'cancel',
                                'position':0, 'duration':2, 'epoch':0}})
                    pending = pool.submit(prepare)
                    assert stall.wait(5), 'decoder never requested media'
                    began = time.monotonic()
                    call('stop', 'cancel')
                    response = pending.result(timeout=3)
                    assert not response.get('result', {}).get('ok'), response
                    assert time.monotonic() - began < 2, 'stop blocked behind decoding'
                    assert not any(p.name().lower() == 'ffmpeg.exe' for p in psutil.Process(process.pid).children(recursive=True)), 'decoder process survived cancellation'
                report['checks'].append('stop cancels blocked decoder through independent Core connection')

            # 只上传公开的 Wikipedia 单词音频，不上传用户媒体。
            public = 'https://upload.wikimedia.org/wikipedia/commons/2/2e/En-us-Wikipedia.ogg'
            cloud_source = public
            if args.public_audio:
                shutil.copyfile(args.public_audio, fixture / 'public.ogg')
                cloud_source = f'http://127.0.0.1:{server.server_port}/public.ogg'
            report['public_audio'] = {'source': public, 'served_from_local_copy':bool(args.public_audio)}
            start('cloud', cloud_source)
            wait(lambda: poll('cloud')['status'] == 'running', 'cloud not running')
            audio = call('prepare_audio', 'cloud', epoch=0, position=0, duration=2)
            pcm = base64.b64decode(audio['audio'])
            began = time.monotonic()
            for offset in range(0, len(pcm), 8000):
                chunk = pcm[offset:offset+8000]
                call('audio', 'cloud', epoch=0, position=offset/32000, rate=1,
                     audio=base64.b64encode(chunk).decode())
                time.sleep(len(chunk)/32000)
            call('commit', 'cloud', epoch=0, start=0, end=len(pcm)/32000)
            done = wait(lambda: (r if any(e['type']=='ready' for e in r['events']) else None)
                        if (r := poll('cloud')) else None, 'cloud did not complete window', 30)
            cues = [e for e in done['events'] if e['type']=='cue' and e.get('final')]
            assert any('wikipedia' in e['text'].lower() for e in cues if e['role']=='source'), cues
            assert any('维基' in e['text'] for e in cues if e['role']=='translation'), cues
            report['cloud'] = {'audio_seconds':len(pcm)/32000, 'elapsed_seconds':round(time.monotonic()-began, 3),
                               'events':done['events']}
            call('reset', 'cloud', epoch=1)
            wait(lambda: poll('cloud')['status']=='running', 'cloud did not reconnect after seek')
            call('audio', 'cloud', epoch=1, position=0, rate=1, audio=base64.b64encode(pcm[:8000]).decode())
            call('stop', 'cloud')
            wait(lambda: poll('cloud')['status']=='stopped', 'cloud finish failed')
            report['checks'].append('real QwenCloud manual append/commit returns original, translation and ready')
            report['checks'].append('real cloud seek reconnect and stopping during uncommitted audio finish cleanly')
            assert not list(root.rglob('.hls-tasks')), 'prefetch created task cache'
            assert send(ipc, {'type':'snapshot','request_id':3})['tasks'] == [], 'prefetch created download tasks'
            report['checks'].append('no download task or temporary segment directory created')
        report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc().replace(key, '[redacted]')
    finally:
        key = ''
        if process:
            process.terminate()
            process.wait(timeout=15)
        if server:
            server.shutdown()
            server.server_close()
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--tools', type=Path, required=True)
    parser.add_argument('--key-file', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--stage', choices=('all', 'cloud'), default='all')
    parser.add_argument('--public-audio', type=Path, help='Unmodified Wikimedia audio copy for VMs without Wikimedia connectivity')
    raise SystemExit(run(parser.parse_args()))
