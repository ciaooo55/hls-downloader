"""Sandbox regression for real libmpv playback and closing/reopening its window."""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
import traceback


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
        command("set pause no")
        time.sleep(1)
        current = metadata()
        assert current["position_seconds"] > report["seek"]["position_seconds"], report
        assert current["paused"] is False and abs(current["speed"] - 1.5) < .01, current
        report["resume_advances"] = True
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
        report["passed"] = True
    except Exception:
        report["error"] = traceback.format_exc()
    finally:
        process.terminate()
        process.wait(timeout=10)
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
