"""Validate the active portable installation without creating a second profile."""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import threading
import time
from urllib.request import Request, urlopen
import uuid

import psutil

from msi_checkpoint_fixture import BLOCK, PAYLOAD_SIZE, RangeHandler, Server
from smoke_v7_native_host import responses
from smoke_v7_presenter import native_message, visible_window
from smoke_v7_transfer_performance import free_port


class Pipe:
    def __init__(self, pipe_name=r"\\.\pipe\HLSDownloader.v7"):
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                                           wintypes.LPVOID, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        self.kernel.CreateFileW.restype = wintypes.HANDLE
        for name in ("ReadFile", "WriteFile"):
            getattr(self.kernel, name).argtypes = [wintypes.HANDLE, wintypes.LPVOID, wintypes.DWORD,
                                                   ctypes.POINTER(wintypes.DWORD), wintypes.LPVOID]
        self.kernel.PeekNamedPipe.argtypes = [wintypes.HANDLE, wintypes.LPVOID, wintypes.DWORD,
                                             wintypes.LPVOID, ctypes.POINTER(wintypes.DWORD), wintypes.LPVOID]
        self.kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        self.handle = self.kernel.CreateFileW(pipe_name, 0xc0000000, 0, None, 3, 0, None)
        if self.handle == ctypes.c_void_p(-1).value:
            raise ctypes.WinError(ctypes.get_last_error())

    def read(self, size):
        data = bytearray()
        deadline = time.monotonic() + 10
        while len(data) < size:
            available = wintypes.DWORD()
            if not self.kernel.PeekNamedPipe(self.handle, None, 0, None, ctypes.byref(available), None):
                raise ctypes.WinError(ctypes.get_last_error())
            if not available.value:
                if time.monotonic() >= deadline:
                    raise TimeoutError("Core named-pipe response exceeded 10 seconds")
                time.sleep(.01)
                continue
            buffer = ctypes.create_string_buffer(min(size-len(data), available.value))
            read = wintypes.DWORD()
            if not self.kernel.ReadFile(self.handle, buffer, len(buffer), ctypes.byref(read), None):
                raise ctypes.WinError(ctypes.get_last_error())
            if not read.value:
                raise EOFError("Core closed its pipe")
            data.extend(buffer.raw[:read.value])
        return bytes(data)

    def request(self, message):
        payload = json.dumps(message, separators=(",", ":")).encode()
        framed = struct.pack("<I", len(payload)) + payload
        sent = wintypes.DWORD()
        buffer = ctypes.create_string_buffer(framed)
        if not self.kernel.WriteFile(self.handle, buffer, len(framed), ctypes.byref(sent), None):
            raise ctypes.WinError(ctypes.get_last_error())
        if sent.value != len(framed):
            raise RuntimeError("Core pipe write was truncated")
        size = struct.unpack("<I", self.read(4))[0]
        if not 0 < size <= 4*1024*1024:
            raise ValueError("Core response frame is invalid")
        return json.loads(self.read(size))

    def close(self):
        self.kernel.CloseHandle(self.handle)


def runtime_processes(root):
    result = []
    for process in psutil.process_iter(["pid", "name", "exe"]):
        path = process.info.get("exe")
        if path and Path(path).is_relative_to(root) and process.info["name"].startswith("HLSDownloader"):
            result.append(process.info)
    return result


def wait(predicate, description, timeout=60):
    deadline = time.monotonic()+timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.1)
    raise TimeoutError(description)


def run(args):
    root = args.runtime.resolve()
    evidence = args.report.parent.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    if not (root / "portable").is_file():
        raise ValueError("The active installation must have its portable marker")
    environment = {k: v for k, v in os.environ.items() if not k.startswith("HLS_")}
    resources = root / "app" / "resources"
    result = {"passed": False, "runtime": str(root), "steps": []}
    pipe = None
    host = None
    server = None
    task_id = None

    def check(name, passed, actual):
        result["steps"].append({"name": name, "passed": bool(passed), "actual": actual})
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps({"step": name, "passed": bool(passed)}, ensure_ascii=False), flush=True)
        if not passed:
            raise AssertionError(f"{name}: {actual}")

    try:
        pings, first, _, _ = responses(resources / "HLSDownloaderNativeHost.exe", environment)
        check("default-native-host-ping", all(p.get("ok") is True for p in pings), {"responses": pings, "first_ms": first})
        pipe = Pipe()
        hello = pipe.request({"type": "hello", "protocol": "hls-downloader-v7-core", "version": 1})
        check("default-core-pipe", hello.get("type") == "hello", hello)
        tasks = pipe.request({"type": "snapshot", "request_id": 1})
        check("preserved-empty-profile", tasks.get("tasks") == [], tasks)
        settings = pipe.request({"type": "load_settings", "request_id": 2})
        result["original_download_dir"] = settings.get("download_dir")
        changed = pipe.request({"type": "store_setting", "request_id": 3, "key": "download_dir", "value": str(root / "downloads")})
        check("portable-download-directory", changed.get("download_dir") == str(root / "downloads"), {"download_dir": changed.get("download_dir")})

        # 只重启本安装的工作台，Core 与用户数据库保持运行。
        current_ui = [p for p in runtime_processes(root) if p["name"] == "HLSDownloader.exe"]
        stopping = []
        for info in current_ui:
            try:
                process = psutil.Process(info["pid"])
                process.terminate()
                stopping.append(process)
            except psutil.NoSuchProcess:
                pass
        _, alive = psutil.wait_procs(stopping, timeout=10)
        if alive:
            raise RuntimeError("The current workbench could not be stopped")
        port, token = free_port(), uuid.uuid4().hex
        ui_env = {**environment, "HLS_UI_TEST_API": "1", "HLS_UI_TEST_PORT": str(port), "HLS_UI_TEST_TOKEN": token}
        with (evidence / "portable-ui.log").open("wb") as log:
            subprocess.Popen([str(root / "HLSDownloader.exe")], cwd=root, env=ui_env, stdout=log, stderr=log)

        def ui(path, raw=False):
            with urlopen(Request(f"http://127.0.0.1:{port}/{path}", headers={"X-HLS-Test-Token": token}), timeout=5) as response:
                data = response.read()
            return data if raw else json.loads(data)

        def ready():
            try:
                state, window = ui("state"), ui("window")
                return {"state": state, "window": window} if window.get("showing") and "已连接" in state.get("engineText", "") else None
            except OSError:
                return None

        ready_state = wait(ready, "Portable workbench did not connect", 90)
        check("visible-connected-workbench", True, ready_state)
        (evidence / "portable-workbench.png").write_bytes(ui("screenshot", True))
        duplicate = subprocess.Popen([str(root / "HLSDownloader.exe")], cwd=root, env=environment)
        code = duplicate.wait(timeout=30)
        check("workbench-single-instance", code == 0, {"second_launcher_exit": code})

        server = Server(("127.0.0.1", 0), RangeHandler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        url = f"http://127.0.0.1:{server.server_port}/portable-validation.bin"
        created = pipe.request({"type": "command", "request_id": 4, "command": {"kind": "create_task", "spec": {
            "url": url, "resource_kind": "file", "filename": "portable-validation.bin", "title": "Portable validation",
            "expected_size": PAYLOAD_SIZE, "concurrency": 1}}})
        event = next((e for e in created.get("events", []) if e["event"].get("kind") == "task_created"), None)
        check("create-download-task", event is not None, created)
        task_id = event["event"]["snapshot"]["task_id"]

        def task():
            return next(t for t in pipe.request({"type": "snapshot", "request_id": 5})["tasks"] if t["task_id"] == task_id)

        def action(value):
            return pipe.request({"type": "command", "request_id": 6, "command": {"kind": "task_action", "task_id": task_id, "action": value}})

        action("start")
        wait(lambda: task()["downloaded_bytes"] >= 256*1024, "Download never progressed")
        action("pause")
        paused = wait(lambda: (t if (t := task())["status"] == "paused" and t["active_workers"] == 0 else None), "Pause never settled")
        check("pause-with-checkpoint", paused["downloaded_bytes"] > 0, paused)
        action("resume")
        completed = wait(lambda: (t if (t := task())["status"] in ("completed", "failed") else None), "Resume never completed", 90)
        check("resumed-download-completed", completed["status"] == "completed", completed)
        output = Path(completed["output_path"])
        expected = hashlib.sha256(BLOCK*(PAYLOAD_SIZE//len(BLOCK))).hexdigest()
        actual = hashlib.sha256(output.read_bytes()).hexdigest()
        check("32mib-integrity", actual == expected and output.stat().st_size == PAYLOAD_SIZE and output.is_relative_to(root), {"sha256": actual, "size": output.stat().st_size, "path": str(output)})
        # 工作台必须收到实际 Core 状态，而非仅在协议侧完成。
        bounds = ui("state")["controlBounds"]["sidebar.status.已完成"]
        action_body = json.dumps({"type": "click", "x": (bounds[0]+bounds[2])//2, "y": (bounds[1]+bounds[3])//2}).encode()
        with urlopen(Request(f"http://127.0.0.1:{port}/action", data=action_body, headers={"X-HLS-Test-Token": token, "Content-Type": "application/json"}), timeout=5) as response:
            response.read()
        rendered = wait(lambda: (s if f"taskrow.{task_id}" in (s := ui("state")).get("controlBounds", {}) else None), "Completed task absent from workbench")
        check("completion-rendered-in-workbench", True, rendered)
        (evidence / "portable-download-completed.png").write_bytes(ui("screenshot", True))

        host = subprocess.Popen([str(resources / "HLSDownloaderNativeHost.exe")], env=environment,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        offer = native_message(host, {"op": "offer", "resource": {"url": url+"?confirm=1", "resource_kind": "file",
            "filename": "confirmation.bin", "title": "Portable Presenter", "size": PAYLOAD_SIZE, "client_request_id": uuid.uuid4().hex}})
        check("native-host-offer", offer.get("ok") is True and bool(offer.get("handoff", {}).get("id")), offer)
        presenter = wait(lambda: next((p for p in runtime_processes(root) if p["name"] == "HLSDownloaderPresenter.exe" and visible_window(p["pid"], "确认下载")), None), "Presenter not visible", 30)
        check("presenter-visible", True, presenter)
        rejected = native_message(host, {"op": "reject_handoff", "handoff_id": offer["handoff"]["id"]})
        check("presenter-reject", rejected.get("ok") is True, rejected)
        wait(lambda: not visible_window(presenter["pid"], "确认下载"), "Presenter did not hide", 15)

        action("delete_files")
        remaining = pipe.request({"type": "snapshot", "request_id": 7})
        check("test-task-and-file-cleaned", not remaining["tasks"] and not output.exists(), remaining)
        processes = runtime_processes(root)
        check("one-core-one-visible-workbench-one-presenter", sum(p["name"] == "HLSDownloaderEngine.exe" for p in processes) == 1
              and sum(p["name"] == "HLSDownloader.exe" and visible_window(p["pid"], "HLS Downloader") for p in processes) == 1
              and sum(p["name"] == "HLSDownloaderPresenter.exe" for p in processes) == 1, processes)
        check("portable-ui-state-files", (root / "data/ui/workbench.lock").is_file() and (root / "data/ui/window-geometry.properties").is_file(), str(root / "data/ui"))
        result["passed"] = True
    except Exception as error:
        result["error"] = str(error)
        print(str(error), flush=True)
    finally:
        if host:
            host.stdin.close()
            try:
                host.wait(timeout=5)
            except subprocess.TimeoutExpired:
                host.terminate()
        if pipe:
            pipe.close()
        if server:
            server.shutdown()
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
