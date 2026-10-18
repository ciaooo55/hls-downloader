"""沙盒：公开长录音的生产插件/Core 连续预取、拖动、倍速和停止验收。"""
import argparse
import contextlib
import functools
from http.server import ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import traceback

import psutil

import smoke_qwencloud_public_subtitles as browser
from smoke_extension_subtitles import QuietHandler
from smoke_extension_takeover import _registered_host
from smoke_v7_portable_app import Pipe


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Run only in the acceptance VM'
    report = {'passed': False, 'checks': [], 'samples': [], 'source':
              'https://commons.wikimedia.org/wiki/File:JFK_inaugural_address.ogg'}
    driver = server = popup = page = sid = None
    temporary = tempfile.TemporaryDirectory(prefix='hls-cloud-continuous-')
    key = args.key_file.read_text(encoding='utf-8-sig').strip() if args.key_file else ''
    if args.key_file:
        args.key_file.unlink()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    try:
        browser._configure_core(key)
        key = ''
        root = Path(temporary.name)
        media = root / 'speech.mp4'
        ffmpeg = args.runtime / 'app/resources/ffmpeg.exe'
        # 截取公开历史录音的前 96 秒；实际解码、发送仍由生产 Core 和插件执行。
        prepared = subprocess.run([str(ffmpeg), '-nostdin', '-y', '-v', 'error',
            '-f', 'lavfi', '-i', 'color=c=0x263344:s=640x360:r=10', '-i', str(args.speech),
            '-t', '96', '-c:v', 'libx264', '-preset', 'ultrafast', '-c:a', 'aac',
            '-movflags', '+faststart', str(media)], capture_output=True, timeout=60)
        assert prepared.returncode == 0, prepared.stderr.decode(errors='replace')
        (root / 'index.html').write_text('<!doctype html><meta charset="utf-8">'
            '<title>公开录音连续字幕验收</title><video id="speech" controls '
            'width="640" height="360" src="speech.mp4"></video>', encoding='utf-8')
        server = ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(QuietHandler, directory=str(root)))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        extension = args.extension.resolve()
        manifest = root / 'native-host.json'
        eid = json.loads((extension / 'manifest.json').read_text(encoding='utf-8'))['browser_specific_settings']['gecko']['id']
        manifest.write_text(json.dumps({'name':'com.ciaooo55.hls_downloader', 'description':'sandbox acceptance',
            'path':str(args.runtime / 'app/resources/HLSDownloaderNativeHost.exe'), 'type':'stdio',
            'allowed_extensions':[eid]}), encoding='utf-8')
        browser._find_firefox = lambda: args.firefox
        options = argparse.Namespace(browser='firefox', runtime=str(args.runtime),
            driver=str(args.driver), headed=True)
        with _registered_host(manifest, 'firefox'):
            driver, popup_url, _ = browser._open_driver(options, extension, root / 'profile')
            driver.get(f'http://127.0.0.1:{server.server_port}/index.html')
            page = driver.current_window_handle
            browser._wait(lambda: driver.execute_script('return document.querySelector("#speech").readyState >= 2'), 'media not loaded')
            driver.switch_to.new_window('tab')
            popup = driver.current_window_handle
            driver.get(popup_url)
            browser._wait(lambda: browser._extension_state(driver).get('players'), 'player not inventoried')
            player = browser._extension_state(driver)['players'][0]
            browser._set_settings(driver, {'enabled':True, 'source':'en', 'target':'zh',
                'showSource':True, 'prefetch':True})
            browser._wait(lambda: driver.execute_script('return !document.querySelector("[data-subtitle-start]")?.disabled'), 'settings not ready')

            def page_eval(code):
                driver.switch_to.window(page)
                return driver.execute_script(code)

            page_eval('document.querySelector("#speech").play();')
            driver.switch_to.window(popup)
            started = browser._require_ok(browser._message(driver, {'type':'subtitle-start',
                'frameId':player['frameId'], 'mediaId':player['mediaId']}), 'start prefetch')
            report['start'] = started
            driver.switch_to.window(page)
            start = time.monotonic()
            ready_seen = {}
            final_cues = {}
            sequence = 0

            def sample():
                nonlocal sid, sequence
                state = page_eval('const v=document.querySelector("#speech");return {time:v.currentTime,paused:v.paused,rate:v.playbackRate, captions:[...document.querySelectorAll("[data-hls-subtitle]")].map(h=>({text:h.shadowRoot?.querySelector(".translation")?.textContent||"",status:h.shadowRoot?.querySelector(".status")?.textContent||""}))}')
                driver.switch_to.window(popup)
                session = browser._extension_state(driver).get('session') or {}
                sid = session.get('id') or sid
                assert session.get('status') != 'failed', session
                assert session.get('capture') == 'prefetch', session
                core = browser._core_poll_popup(driver, popup, page, sid, sequence) if sid else {}
                assert core.get('status') != 'failed', core
                sequence = core.get('latest_sequence', sequence)
                state.update(elapsed=round(time.monotonic()-start, 3), epoch=session.get('epoch'),
                    status=session.get('status'))
                for event in core.get('events', []):
                    if event['type'] == 'ready':
                        ready_seen[(event['epoch'], event['start'], event['end'])] = event
                    elif event['type'] == 'cue' and event.get('final'):
                        final_cues[(event['epoch'], event['sequence'])] = event
                state['ready'] = list(ready_seen.values())
                if not state['paused'] and 'first_resume_seconds' not in report:
                    report['first_resume_seconds'] = state['elapsed']
                report['samples'].append(state)
                args.report.with_suffix('.progress.json').write_text(json.dumps(state, ensure_ascii=False), encoding='utf-8')
                driver.switch_to.window(page)
                return state, core

            def wait_sample(predicate, description, timeout):
                deadline = time.monotonic() + timeout
                while time.monotonic() < deadline:
                    state = sample()[0]
                    if predicate(state):
                        return state
                    time.sleep(.5)
                raise AssertionError(description)

            # 初次准备应暂停，至少三个云端提交完成后实际播放仍有字幕。
            initial, _ = sample()
            assert initial['paused'], initial
            wait_sample(lambda s: s['time'] > 18,
                'continuous prefetch did not advance through three windows', 110)
            assert len(ready_seen) >= 3, ready_seen
            assert any(c['role'] == 'source' and c['text'].strip() for c in final_cues.values()), final_cues
            assert any(c['role'] == 'translation' and any('\u4e00' <= ch <= '\u9fff' for ch in c['text'])
                       for c in final_cues.values()), final_cues
            assert any(c['text'] for s in report['samples'] for c in s['captions']), 'completed translations never appeared on the player'
            report['checks'].append('real cloud completes multiple windows while production browser resumes and advances')
            report['before_seek'] = sample()
            driver.save_screenshot(str(args.report.with_suffix('.before-seek.png')))

            # 倍速不应改变原音频时间轴或新建 epoch。
            epoch = sample()[0]['epoch']
            page_eval('document.querySelector("#speech").playbackRate=1.5;')
            time.sleep(1)
            assert sample()[0]['epoch'] == epoch, 'rate change reset the original-speed audio timeline'
            report['checks'].append('1.5x playback retains the original audio epoch')

            # 跳到尚未翻译的位置，必须重连，准备好该时间段再恢复播放。
            page_eval('const v=document.querySelector("#speech");v.currentTime=72;v.play();')
            wait_sample(lambda s: s['epoch'] > epoch, 'seek did not create a new epoch', 15)
            resumed = wait_sample(lambda s: s['time'] > 73 and not s['paused'],
                'seek did not resume after translated audio was ready', 80)
            assert any(e['epoch'] == resumed['epoch'] and e['start'] >= 71.9 for e in ready_seen.values()), resumed
            report['checks'].append('uncached seek reconnects real cloud and resumes only after new-epoch completion')
            report['after_seek'] = sample()
            driver.save_screenshot(str(args.report.with_suffix('.after-seek.png')))

            driver.switch_to.window(popup)
            browser._require_ok(browser._message(driver, {'type':'subtitle-stop'}), 'stop')
            browser._wait(lambda: browser._core_poll_popup(driver, popup, page, sid)['status'] == 'stopped',
                'real cloud did not stop', timeout=25)
            page_eval('document.querySelector("#speech").pause();')
            report['checks'].append('production stop ends the cloud session')
            with contextlib.closing(Pipe()) as pipe:
                assert pipe.request({'type':'snapshot','request_id':1})['tasks'] == [], 'subtitle preparation created a download task'
            assert not list(Path(os.environ['HLS_V7_DOWNLOAD_DIR']).rglob('.hls-tasks')), 'subtitle preparation created task cache'
            assert not any(p.name().lower() == 'ffmpeg.exe' for p in psutil.process_iter()), 'FFmpeg survived stop'
            report['checks'].append('no download task, segment cache or decoder remains after stop')
            report['completed_windows'] = list(ready_seen.values())
            report['final_cues'] = list(final_cues.values())
            report['passed'] = True
    except Exception:
        report['error'] = traceback.format_exc().replace(key, '[redacted]') if key else traceback.format_exc()
        if driver:
            if popup and sid:
                with contextlib.suppress(Exception):
                    driver.switch_to.window(popup)
                    browser._message(driver, {'type':'subtitle-stop'})
            with contextlib.suppress(Exception):
                driver.save_screenshot(str(args.report.with_suffix('.failure.png')))
    finally:
        if driver:
            with contextlib.suppress(Exception):
                driver.quit()
        if server:
            server.shutdown()
            server.server_close()
        temporary.cleanup()
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('runtime', 'extension', 'firefox', 'driver', 'speech', 'report'):
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--key-file', type=Path, help='One-use credential file; omitted to use configured Core credentials')
    raise SystemExit(run(parser.parse_args()))
