"""Hosted Windows: actual tray Quit must checkpoint an active download."""
from __future__ import annotations

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
import uuid

from msi_checkpoint_fixture import BLOCK, PAYLOAD_SIZE, RangeHandler, Server, file_hash
from v7_core_ipc import connect, free_port, send_frame, snapshot, wait_task
from validation_workspace import test_tmp


def tray_window(process: subprocess.Popen) -> int:
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    user32.FindWindowExW.argtypes = [wintypes.HWND, wintypes.HWND, wintypes.LPCWSTR, wintypes.LPCWSTR]
    user32.FindWindowExW.restype = wintypes.HWND
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        assert process.poll() is None, process.returncode
        for parent in (wintypes.HWND(-3), None):
            window = None
            while True:
                window = user32.FindWindowExW(parent, window, "HLSDownloader.v7.Tray", None)
                if not window:
                    break
                pid = wintypes.DWORD()
                user32.GetWindowThreadProcessId(window, ctypes.byref(pid))
                if pid.value == process.pid:
                    return window
        time.sleep(0.05)
    raise TimeoutError("Actual Core tray window did not become ready")


def main() -> int:
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_OS") != "Windows":
        raise RuntimeError("Tray acceptance only runs on an isolated GitHub Windows runner")
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    root = test_tmp() / ("tray-checkpoint-" + uuid.uuid4().hex)
    root.mkdir()
    origin = Server(("127.0.0.1", 0), RangeHandler)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    port = free_port()
    downloads = root / "downloads"
    env = os.environ.copy()
    env.update({
        "HLS_V7_PIPE": rf"\\.\pipe\HLSTrayCheckpoint-{uuid.uuid4().hex}",
        "HLS_V7_CORE_TCP": "1", "HLS_V7_CORE_BIND": f"127.0.0.1:{port}",
        "HLS_V7_DATA_DIR": str(root / "data"), "HLS_V7_DOWNLOAD_DIR": str(downloads),
    })
    result = {"passed": False, "engine_sha256": hashlib.sha256(args.engine.read_bytes()).hexdigest()}
    process = None
    try:
        with (root / "core.log").open("wb") as log:
            process = subprocess.Popen([str(args.engine)], env=env, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=subprocess.STDOUT,
                                       creationflags=subprocess.CREATE_NO_WINDOW)
            window = tray_window(process)
            with connect(port) as ipc:
                response = send_frame(ipc, {"type": "store_setting", "request_id": 1,
                                           "key": "resume_interrupted_on_startup", "value": False})
                assert response["type"] != "error", response
                created = send_frame(ipc, {"type": "command", "request_id": 2, "command": {
                    "kind": "create_task", "spec": {
                        "url": f"http://127.0.0.1:{origin.server_port}/fixture.bin",
                        "filename": "fixture.bin", "resource_kind": "file", "concurrency": 1,
                        "expected_size": PAYLOAD_SIZE,
                    }}})
                event = next(item["event"] for item in created["events"]
                             if item["event"]["kind"] == "task_created")
                task_id = event["snapshot"]["task_id"]
                before = wait_task(ipc, task_id, lambda task: task["downloaded_bytes"] >= 256 * 1024
                                   and task["active_workers"] > 0, 10)
            user32 = ctypes.WinDLL("user32", use_last_error=True)
            user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
            assert user32.PostMessageW(window, 0x0111, 4, 0), ctypes.get_last_error()
            process.wait(timeout=15)
            assert process.returncode == 0, process.returncode
            # The old process::exit path leaves this as "run". Require the
            # actual worker-stop boundary, not merely crash recovery on reopen.
            control = downloads / ".hls-tasks" / task_id / "control"
            assert control.read_text(encoding="utf-8").strip() == "pause", control
            process = subprocess.Popen([str(args.engine), "--no-tray"], env=env,
                                       stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                       creationflags=subprocess.CREATE_NO_WINDOW)
            with connect(port) as ipc:
                restored = snapshot(ipc, task_id, 100)
                assert restored["status"] == "paused", restored
                assert restored["downloaded_bytes"] >= before["downloaded_bytes"], restored
                resumed = send_frame(ipc, {"type": "command", "request_id": 101, "command": {
                    "kind": "task_action", "task_id": task_id, "action": "resume"}})
                assert resumed["type"] == "events", resumed
                completed = wait_task(ipc, task_id, lambda task: task["status"] in {"completed", "failed"}, 110)
                assert completed["status"] == "completed", completed
                expected = hashlib.sha256(BLOCK * (PAYLOAD_SIZE // len(BLOCK))).hexdigest()
                output = downloads / "fixture.bin"
                actual = file_hash(output)
                assert output.stat().st_size == PAYLOAD_SIZE and actual == expected, actual
                stopped = send_frame(ipc, {"type": "command", "request_id": 200,
                                            "command": {"kind": "shutdown"}})
                assert stopped["type"] == "events", stopped
            process.wait(timeout=15)
            assert process.returncode == 0, process.returncode
            result.update(passed=True, before=before, restored=restored,
                          file_sha256=actual, expected_sha256=expected)
    except Exception as error:
        result.update(error=repr(error), traceback=traceback.format_exc())
    finally:
        if process is not None and process.poll() is None:
            try:
                with connect(port) as ipc:
                    send_frame(ipc, {"type": "command", "request_id": 300,
                                     "command": {"kind": "shutdown"}})
                process.wait(timeout=15)
            except Exception as error:
                result["cleanup_error"] = repr(error)
                process.kill()
                process.wait(timeout=5)
        origin.shutdown()
        origin.server_close()
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
