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
local function save(final)
    local result = {}
    for _, name in ipairs({'hwdec', 'hwdec-current', 'current-vo', 'current-gpu-context',
        'video-codec', 'video-params', 'frame-drop-count', 'decoder-frame-drop-count'}) do
        result[name] = mp.get_property_native(name)
    end
    result.hwdec = mp.get_property('hwdec')
    result.final = final
    local file = io.open(%s, 'w')
    if file then file:write(utils.format_json(result)); file:close() end
end
local timer = mp.add_periodic_timer(1, function() save(false) end)
mp.register_script_message('render-report-finalize', function()
    timer:kill()
    save(true)
end)
''' % json.dumps(diagnostic.as_posix()), encoding='utf-8')
    report = {'passed': False, 'environment': args.environment, 'vo_override': args.vo,
              'engine_sha256': hashlib.sha256(args.engine.read_bytes()).hexdigest(),
              'media_sha256': hashlib.sha256(args.media.read_bytes()).hexdigest()}
    env = {**os.environ, 'HLS_V7_LIBMPV': str(args.libmpv.resolve())}
    env.pop('HLS_V7_PLAYER_NULL', None)
    with args.report.with_suffix('.stderr.log').open('wb') as log:
        process = subprocess.Popen([str(args.engine.resolve()), '--player-process'],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
                                   env=env, text=True)
        try:
            if args.vo:
                response = exchange(process, {'op': 'mpv', 'command': f'set vo {args.vo}'})
                assert response.get('ok'), response
            for command in (f'load-script {json.dumps(script.as_posix())}',
                            'set loop-file inf',
                            f'loadfile {json.dumps(args.media.resolve().as_posix())} replace'):
                response = exchange(process, {'op': 'mpv', 'command': command})
                assert response.get('ok'), response
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                state = exchange(process, {'op': 'metadata'})
                if state.get('value', {}).get('position_available') and diagnostic.exists():
                    break
                time.sleep(.1)
            else:
                raise AssertionError('Actual player did not decode media')
            # Exclude initialization and shader compilation from steady playback sampling.
            time.sleep(2)
            player = psutil.Process(process.pid)
            cpu_before = player.cpu_times()
            started = time.monotonic()
            before = exchange(process, {'op': 'metadata'})['value']
            time.sleep(5)
            after = exchange(process, {'op': 'metadata'})['value']
            elapsed = time.monotonic() - started
            cpu_after = player.cpu_times()
            cpu = cpu_after.user + cpu_after.system - cpu_before.user - cpu_before.system
            report.update(elapsed_seconds=elapsed, cpu_one_core_percent=100*cpu/elapsed,
                          cpu_machine_percent=100*cpu/elapsed/(psutil.cpu_count() or 1),
                          rss_bytes=player.memory_info().rss, before=before, after=after)
            response = exchange(process, {'op': 'mpv', 'command': 'script-message render-report-finalize'})
            assert response.get('ok'), response
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                try:
                    result = json.loads(diagnostic.read_text(encoding='utf-8'))
                    if result.get('final'):
                        report['mpv'] = result
                        break
                except (OSError, json.JSONDecodeError):
                    pass
                time.sleep(.05)
            else:
                raise AssertionError('Player diagnostic did not finish')
            response = exchange(process, {'op': 'stop'})
            assert response.get('ok'), response
            process.kill()
            process.wait(timeout=5)
            assert report['mpv'].get('current-vo'), report
            assert after['position_available'] and after['position_seconds'] > before['position_seconds'], report
            if args.expected_hwdec:
                assert report['mpv'].get('hwdec') == args.expected_hwdec, report
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
    parser.add_argument('--vo', choices=('gpu', 'gpu-next'))
    raise SystemExit(run(parser.parse_args()))
