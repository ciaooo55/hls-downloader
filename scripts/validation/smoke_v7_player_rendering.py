"""Sandbox-only comparison of the actual player subprocess and decoder selection."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

import psutil

from smoke_v7_player_process import exchange


def run(args):
    assert os.environ.get('HLS_VM_ACCEPTANCE') == '1', 'Sandbox only'
    args.report.parent.mkdir(parents=True, exist_ok=True)
    diagnostic = args.report.with_suffix('.mpv.json')
    script = args.report.with_suffix('.lua')
    script.write_text('''local mp = require 'mp'
local utils = require 'mp.utils'
local function save(sample)
    local result = {}
    for _, name in ipairs({'hwdec', 'hwdec-current', 'current-vo', 'current-gpu-context',
        'video-codec', 'video-params', 'osd-dimensions', 'estimated-vf-fps',
        'frame-drop-count', 'decoder-frame-drop-count', 'dither', 'scale', 'dscale',
        'hdr-compute-peak', 'correct-downscaling', 'linear-downscaling', 'cscale'}) do
        result[name] = mp.get_property_native(name)
    end
    result.hwdec = mp.get_property('hwdec')
    result.sample = sample
    local file = io.open(%s, 'w')
    if file then file:write(utils.format_json(result)); file:close() end
end
mp.register_script_message('render-report-snapshot', save)
''' % json.dumps(diagnostic.as_posix()), encoding='utf-8')
    report = {'passed': False, 'environment': args.environment, 'vo_override': args.vo,
              'warmup_seconds': 5,
              'profile_override': args.profile,
              'option_overrides': args.mpv_option,
              'engine_sha256': hashlib.sha256(args.engine.read_bytes()).hexdigest(),
              'media_sha256': hashlib.sha256(args.media.read_bytes()).hexdigest()}
    env = {**os.environ, 'HLS_V7_LIBMPV': str(args.libmpv.resolve())}
    env.pop('HLS_V7_PLAYER_NULL', None)
    with args.report.with_suffix('.stderr.log').open('wb') as log:
        process = subprocess.Popen([str(args.engine.resolve()), '--player-process'],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
                                   env=env, text=True, encoding='utf-8')
        try:
            def snapshot(sample):
                response = exchange(process, {'op': 'mpv', 'command': f'script-message render-report-snapshot {sample}'})
                assert response.get('ok'), response
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    try:
                        result = json.loads(diagnostic.read_text(encoding='utf-8'))
                        if result.get('sample') == sample:
                            return result
                    except (OSError, json.JSONDecodeError):
                        pass
                    time.sleep(.05)
                raise AssertionError(f'Player diagnostic did not finish: {sample}')

            if args.profile:
                response = exchange(process, {'op': 'mpv', 'command': f'apply-profile {args.profile}'})
                assert response.get('ok'), response
            for option in args.mpv_option:
                name, separator, value = option.partition('=')
                assert separator and name.isascii() and name.replace('-', '').isalnum(), option
                response = exchange(process, {'op': 'mpv', 'command': f'set {name} {json.dumps(value)}'})
                assert response.get('ok'), response
            if args.vo:
                response = exchange(process, {'op': 'mpv', 'command': f'set vo {args.vo}'})
                assert response.get('ok'), response
            for command in (f'load-script {json.dumps(script.as_posix())}',
                            'set geometry 960x540',
                            'set loop-file inf',
                            f'loadfile {json.dumps(args.media.resolve().as_posix())} replace'):
                response = exchange(process, {'op': 'mpv', 'command': command})
                assert response.get('ok'), response
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                state = exchange(process, {'op': 'metadata'})
                if state.get('value', {}).get('position_available'):
                    break
                time.sleep(.1)
            else:
                raise AssertionError('Actual player did not decode media')
            # Exclude initialization and shader compilation from steady playback sampling.
            time.sleep(5)
            player = psutil.Process(process.pid)
            render_before = snapshot('before')
            cpu_before = player.cpu_times()
            started = time.monotonic()
            before = exchange(process, {'op': 'metadata'})['value']
            time.sleep(10)
            after = exchange(process, {'op': 'metadata'})['value']
            elapsed = time.monotonic() - started
            cpu_after = player.cpu_times()
            cpu = cpu_after.user + cpu_after.system - cpu_before.user - cpu_before.system
            report.update(elapsed_seconds=elapsed, cpu_one_core_percent=100*cpu/elapsed,
                          cpu_machine_percent=100*cpu/elapsed/(psutil.cpu_count() or 1),
                          rss_bytes=player.memory_info().rss, before=before, after=after)
            report['mpv'] = snapshot('after')
            report['mpv_before'] = render_before
            fps = report['mpv'].get('estimated-vf-fps') or 0
            drops = max(0, report['mpv'].get('frame-drop-count', 0) - render_before.get('frame-drop-count', 0))
            expected_frames = fps * elapsed * (before.get('speed') or 1)
            report['sample_dropped_frames'] = drops
            report['sample_drop_ratio'] = drops / expected_frames if expected_frames > 0 else None
            report['playback_ratio'] = (after['position_seconds'] - before['position_seconds']) / elapsed / (before.get('speed') or 1)
            # 可播放与流畅度分开记录；阈值仅用于固定帧率的对照夹具。
            if args.max_drop_ratio is not None:
                assert expected_frames > 0, 'No frame-rate measurement for drop-ratio assertion'
                assert report['sample_drop_ratio'] <= args.max_drop_ratio, f"Dropped frame ratio {report['sample_drop_ratio']:.3f} exceeds {args.max_drop_ratio}"
                assert report['playback_ratio'] >= .9, f"Playback runs at {report['playback_ratio']:.3f} of requested speed"
            response = exchange(process, {'op': 'stop'})
            assert response.get('ok'), response
            process.kill()
            process.wait(timeout=5)
            assert report['mpv'].get('current-vo'), report
            assert after['position_available'] and after['position_seconds'] > before['position_seconds'], report
            if args.expected_hwdec:
                assert report['mpv'].get('hwdec') == args.expected_hwdec, report
            if args.expected_scale:
                assert report['mpv'].get('scale') == args.expected_scale, report
            report['passed'] = True
        except Exception as error:
            report['error'] = str(error)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            script.unlink(missing_ok=True)
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('engine', 'libmpv', 'media', 'report'):
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--environment', required=True)
    parser.add_argument('--expected-hwdec')
    parser.add_argument('--expected-scale')
    parser.add_argument('--vo', choices=('gpu', 'gpu-next', 'direct3d'))
    parser.add_argument('--profile', choices=('fast',))
    parser.add_argument('--max-drop-ratio', type=float)
    parser.add_argument('--mpv-option', action='append', default=[], metavar='NAME=VALUE')
    raise SystemExit(run(parser.parse_args()))
