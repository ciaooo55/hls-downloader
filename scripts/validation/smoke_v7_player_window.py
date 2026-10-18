"""Sandbox regression for real libmpv playback and closing/reopening its window."""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import threading
import time
import traceback
from PIL import Image
import psutil


def run(args):
    assert os.environ.get("HLS_VM_ACCEPTANCE") == "1", "Run only in the acceptance VM"
    engine = args.runtime / "app/resources/HLSDownloaderEngine.exe"
    report = {"passed": False, "engine_sha256": hashlib.sha256(engine.read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    player_log = args.report.with_suffix('.player.log').open('w', encoding='utf-8')
    process = subprocess.Popen(
        [str(engine), "--player-process"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=player_log, text=True, encoding="utf-8",
    )
    host_stop = threading.Event()
    host_ready = threading.Event()
    hosts, host_errors = [], []
    host_thread = None

    def host_windows():
        # 宿主消息循环独立运行，模拟 Compose EDT；IPC 等待不能阻塞 SetParent 消息。
        user = ctypes.windll.user32
        kernel = ctypes.windll.kernel32
        user.SetThreadDpiAwarenessContext.argtypes = [ctypes.c_void_p]
        user.SetThreadDpiAwarenessContext(ctypes.c_void_p(-4))
        kernel.GetModuleHandleW.argtypes = [wintypes.LPCWSTR]
        kernel.GetModuleHandleW.restype = wintypes.HMODULE
        user.CreateWindowExW.argtypes = [wintypes.DWORD, wintypes.LPCWSTR, wintypes.LPCWSTR,
            wintypes.DWORD, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
            wintypes.HWND, wintypes.HMENU, wintypes.HINSTANCE, wintypes.LPVOID]
        user.CreateWindowExW.restype = wintypes.HWND
        user.PeekMessageW.argtypes = [ctypes.POINTER(wintypes.MSG), wintypes.HWND,
                                     wintypes.UINT, wintypes.UINT, wintypes.UINT]
        user.TranslateMessage.argtypes = [ctypes.POINTER(wintypes.MSG)]
        user.DispatchMessageW.argtypes = [ctypes.POINTER(wintypes.MSG)]
        user.DispatchMessageW.restype = wintypes.LPARAM
        user.DestroyWindow.argtypes = [wintypes.HWND]
        try:
            for name in ('Playback main host', 'Playback PiP host'):
                hwnd = user.CreateWindowExW(0, 'STATIC', name, 0x10CF0000,
                    60 + len(hosts)*80, 60 + len(hosts)*80, 720, 400,
                    None, None, kernel.GetModuleHandleW(None), None)
                assert hwnd, 'Could not create playback test host'
                hosts.append(hwnd)
            host_ready.set()
            message = wintypes.MSG()
            while not host_stop.is_set():
                while user.PeekMessageW(ctypes.byref(message), None, 0, 0, 1):
                    user.TranslateMessage(ctypes.byref(message))
                    user.DispatchMessageW(ctypes.byref(message))
                host_stop.wait(.01)
        except Exception:
            host_errors.append(traceback.format_exc())
        finally:
            host_ready.set()
            for hwnd in hosts:
                user.DestroyWindow(hwnd)

    def embedded_child(parent):
        user = ctypes.windll.user32
        callback = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        user.EnumChildWindows.argtypes = [wintypes.HWND, callback, wintypes.LPARAM]
        user.GetParent.argtypes = [wintypes.HWND]
        user.GetParent.restype = wintypes.HWND
        result = []
        @callback
        def visit(hwnd, _):
            pid = wintypes.DWORD()
            user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if pid.value == process.pid and user.GetParent(hwnd) == parent:
                result.append(hwnd)
            return True
        user.EnumChildWindows(parent, visit, 0)
        assert len(result) == 1, result
        return result[0]

    def request(value):
        print(json.dumps({'request': value}), flush=True)
        process.stdin.write(json.dumps(value) + "\n")
        process.stdin.flush()
        response = json.loads(process.stdout.readline())
        print(json.dumps({'response': response}), flush=True)
        assert response["ok"], response
        return response.get("value")

    def command(value):
        return request({"op": "mpv", "command": value})

    def metadata():
        return request({"op": "metadata"})

    def visible_player_window():
        user32 = ctypes.windll.user32
        callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        user32.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
        user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
        user32.IsWindowVisible.argtypes = [wintypes.HWND]
        windows = []
        @callback_type
        def visit(hwnd, _):
            pid = wintypes.DWORD()
            user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if pid.value == process.pid and user32.IsWindowVisible(hwnd):
                windows.append(hwnd)
            return True
        user32.EnumWindows(visit, 0)
        return bool(windows)

    def load():
        path = str(args.media).replace("\\", "/").replace('"', '\\"')
        command(f'loadfile "{path}" replace')
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            current = metadata()
            if current["position_available"] and current["duration_seconds"] > 0:
                return current
            time.sleep(.2)
        raise AssertionError("Video did not become playable")

    try:
        report["initial"] = load()
        assert report["initial"]["audio_tracks"] >= 2, report
        assert report["initial"]["subtitle_tracks"] >= 2, report
        command("keypress SPACE")
        deadline = time.monotonic() + 5
        while metadata()["paused"] is not True and time.monotonic() < deadline:
            time.sleep(.1)
        assert metadata()["paused"] is True, "Default player pause binding did not pause"
        command("keypress SPACE")
        deadline = time.monotonic() + 5
        while metadata()["paused"] is not False and time.monotonic() < deadline:
            time.sleep(.1)
        assert metadata()["paused"] is False, "Default player pause binding did not resume"
        report["keyboard_pause_resume"] = True
        command("set pause yes")
        time.sleep(.5)
        before = metadata()
        time.sleep(1)
        after = metadata()
        assert after["paused"] is True, after
        assert abs(after["position_seconds"] - before["position_seconds"]) < .15, (before, after)
        report["pause_stable"] = True
        target = report["initial"]["duration_seconds"] / 2
        command(f"seek {target} absolute+exact")
        time.sleep(1)
        report["seek"] = metadata()
        assert abs(report["seek"]["position_seconds"] - target) < 2, report
        command("set aid 2")
        command("set sid 2")
        command("set speed 1.5")
        command("set volume 40")
        screenshot = args.report.with_suffix('.png').as_posix().replace('"', '\\"')
        command(f'screenshot-to-file "{screenshot}" window')
        command("set pause no")
        time.sleep(1)
        current = metadata()
        assert current["position_seconds"] > report["seek"]["position_seconds"], report
        assert current["paused"] is False and abs(current["speed"] - 1.5) < .01, current
        report["resume_advances"] = True
        measured = psutil.Process(process.pid)
        before_cpu = measured.cpu_times()
        before_position = current["position_seconds"]
        started = time.monotonic()
        time.sleep(3)
        after_cpu = measured.cpu_times()
        elapsed = time.monotonic() - started
        after_position = metadata()["position_seconds"]
        assert after_position > before_position + 2, "Playback measurement included a stalled player"
        cpu = after_cpu.user + after_cpu.system - before_cpu.user - before_cpu.system
        memory = measured.memory_info()
        report["playing_usage"] = {
            "sample_seconds": elapsed, "position_before": before_position, "position_after": after_position,
            "cpu_one_core_percent": cpu / elapsed * 100,
            "cpu_machine_percent": cpu / elapsed * 100 / psutil.cpu_count(),
            "rss_bytes": memory.rss, "private_bytes": getattr(memory, "private", None),
        }
        assert visible_player_window(), "No visible player before close"
        command("quit")
        deadline = time.monotonic() + 15
        # 此处禁止 metadata 查询：生产界面不轮询，关闭必须由播放器自己处理。
        while visible_player_window() and time.monotonic() < deadline:
            time.sleep(.1)
        assert not visible_player_window(), "Player window cannot close while command input is idle"
        report["closes_without_status_polling"] = True
        deadline = time.monotonic() + 15
        while metadata()["position_available"] and time.monotonic() < deadline:
            time.sleep(.2)
        assert not metadata()["position_available"], "Closed video still has a position"
        assert metadata()["session_active"] is False, "Closed window still reports active"
        report["reopened"] = load()
        assert report["reopened"]["audio_tracks"] == report["initial"]["audio_tracks"], report
        time.sleep(.5)
        command(f'screenshot-to-file "{screenshot}" window')
        with Image.open(args.report.with_suffix('.png')) as frame:
            width, height = frame.size
        # 当前内嵌 bottombar 的播放按钮和右上角关闭按钮，走 mpv 的真实输入处理器。
        command(f"mouse 12 {height - 11}")
        time.sleep(1)
        command(f'screenshot-to-file "{screenshot}" window')
        # OSC 在按下时记录命中控件、抬起时执行；mouse 的单次 press 不产生这对事件。
        command("keydown MBTN_LEFT")
        time.sleep(.1)
        command("keyup MBTN_LEFT")
        deadline = time.monotonic() + 5
        while metadata()["paused"] is not True and time.monotonic() < deadline:
            time.sleep(.1)
        assert metadata()["paused"] is True, "In-video pause button did not pause"
        command("keydown MBTN_LEFT")
        time.sleep(.1)
        command("keyup MBTN_LEFT")
        deadline = time.monotonic() + 5
        while metadata()["paused"] is not False and time.monotonic() < deadline:
            time.sleep(.1)
        assert metadata()["paused"] is False, "In-video play button did not resume"
        report["osc_pause_resume"] = True
        command(f"mouse {width - 7} 10")
        time.sleep(.2)
        command("keydown MBTN_LEFT")
        time.sleep(.1)
        command("keyup MBTN_LEFT")
        deadline = time.monotonic() + 15
        while visible_player_window() and time.monotonic() < deadline:
            time.sleep(.1)
        assert not visible_player_window(), "In-video close button did not close the window"
        report["osc_close_without_status_polling"] = True
        request({'op': 'stop'})
        host_thread = threading.Thread(target=host_windows, daemon=True)
        host_thread.start()
        assert host_ready.wait(10) and len(hosts) == 2 and not host_errors, host_errors
        request({'op': 'embed', 'parent': hosts[0], 'x': 8, 'y': 48, 'w': 640, 'h': 240})
        load()
        command('set pause yes')
        command('set speed 1.5')
        target = metadata()['duration_seconds'] / 2
        command(f'seek {target} absolute+exact')
        deadline = time.monotonic() + 10
        while abs(metadata()['position_seconds']-target) > 1 and time.monotonic() < deadline:
            time.sleep(.1)
        before = metadata()
        child = embedded_child(hosts[0])
        for parent, width, height in [(hosts[1], 400, 180), (hosts[0], 620, 230), (hosts[1], 480, 200)]:
            request({'op': 'hide_embed'})
            assert not ctypes.windll.user32.IsWindowVisible(child)
            request({'op': 'embed', 'parent': parent, 'x': 10, 'y': 48, 'w': width, 'h': height})
            assert embedded_child(parent) == child, 'Host switch recreated the video HWND'
            assert ctypes.windll.user32.IsWindowVisible(child)
            ctypes.windll.user32.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
            rect = wintypes.RECT()
            assert ctypes.windll.user32.GetClientRect(child, ctypes.byref(rect))
            assert rect.right > 0 and rect.bottom > 0, 'Host switch left an empty video surface'
            after = metadata()
            assert after['paused'] is True and abs(after['speed']-1.5) < .01, after
            assert abs(after['position_seconds']-before['position_seconds']) < .15, (before, after)
        report['embedded_main_pip_resize_preserves_session'] = True
        request({'op': 'stop'})
        ctypes.windll.user32.IsWindow.argtypes = [wintypes.HWND]
        assert not ctypes.windll.user32.IsWindow(child), 'Stopping left the video child window'
        report['embedded_close_releases_child'] = True
        report["passed"] = True
    except Exception:
        report["error"] = traceback.format_exc()
        try:
            failed = args.report.with_name(args.report.stem + '-failure.png').as_posix()
            command(f'screenshot-to-file "{failed}" window')
        except Exception:
            pass
    finally:
        process.terminate()
        process.wait(timeout=10)
        host_stop.set()
        if host_thread is not None:
            host_thread.join(timeout=5)
        player_log.close()
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(report, ensure_ascii=False), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--media", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
