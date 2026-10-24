"""Observe real confirmation/progress/completion surfaces across successive downloads.

Core IPC supplies the confirmation decision; this checks rendering, not mouse input.
Only processes and files created by this sandbox-only test are used.
"""
from __future__ import annotations

import argparse
import ctypes
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import shutil
import tempfile
import threading
import time
import uuid

from smoke_v7_portable_app import Pipe
from smoke_v7_presenter import native_message
from smoke_v7_presenter_visual import find_window, print_window, user32, wintypes


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        body = b"repaint-fixture" * 65536
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Content-Type", "application/octet-stream")
        self.end_headers()
        for offset in range(0, len(body), 32768):
            try:
                self.wfile.write(body[offset:offset + 32768])
                self.wfile.flush()
            except ConnectionError:
                # The downloader can replace its initial probe with a transfer.
                return
            time.sleep(.04)


def run(args):
    if os.environ.get("HLS_VM_ACCEPTANCE") != "1":
        raise RuntimeError("This fixture must run inside the acceptance sandbox")
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=True)
    # SQLite and download state stay on the guest's local filesystem; only
    # evidence is written through the host's mapped folder.
    local = Path(tempfile.mkdtemp(prefix="presenter-repaint-"))
    environment = os.environ.copy()
    pipe_name = rf"\\.\pipe\HLSDownloader.repaint-{uuid.uuid4().hex}"
    environment.update(HLS_V7_DATA_DIR=str(local / "profile"), HLS_V7_PIPE=pipe_name,
                       HLS_V7_PRESENTER_READY_FILE=str(local / "ready"))
    processes = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    report = {"passed": False, "dark": args.dark, "samples": []}

    def request(message):
        connection = Pipe(pipe_name)
        try:
            return connection.request(message)
        finally:
            connection.close()

    def capture(process, title, name):
        deadline = time.monotonic() + 15
        while not (hwnd := find_window(process.pid, title)):
            if process.poll() is not None:
                raise RuntimeError(f"Presenter exited: {process.returncode}")
            if time.monotonic() > deadline:
                raise TimeoutError(f"Missing {title}")
            time.sleep(.02)
        time.sleep(.15)
        rect = wintypes.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
        width, height = rect.right - rect.left, rect.bottom - rect.top
        image = print_window(hwnd, width, height)
        if image is None:
            raise AssertionError("PrintWindow did not capture the popup surface")
        image.save(root / f"{name}.png")
        # Primary controls must really be painted in the footer. A nonblank
        # title/progress bar alone cannot pass this regression.
        footer = image.crop((0, height - round(55 * user32.GetDpiForWindow(hwnd) / 96), width, height))
        blue = sum(1 for r, g, b in footer.get_flattened_data() if b > r + 30 and b > g + 15 and b > 110)
        report["samples"].append({"name": name, "size": [width, height],
                                  "dpi": user32.GetDpiForWindow(hwnd), "footer_accent_pixels": blue})
        if title != "下载进度" and blue < 150:
            raise AssertionError(f"{title}: primary footer control is missing")

    try:
        engine = subprocess.Popen([str(args.engine.resolve())], env=environment)
        processes.append(engine)
        presenter = subprocess.Popen([str(args.presenter.resolve())], env=environment)
        processes.append(presenter)
        deadline = time.monotonic() + 30
        while not (local / "ready").exists():
            if presenter.poll() is not None or time.monotonic() > deadline:
                raise RuntimeError("Presenter did not become ready")
            time.sleep(.05)
        (local / "downloads").mkdir()
        configured = request({"type": "store_settings", "request_id": 1, "values": {
            "download_dir": str(local / "downloads"), "legal_terms_accepted": True,
            "dark_mode": args.dark}})
        if configured.get("type") == "error":
            raise AssertionError(configured)
        host = subprocess.Popen([str(args.host.resolve())], env=environment,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        processes.append(host)
        for index in range(3):
            filename = f"重复弹窗验收-{index}-长文件名.bin"
            offered = native_message(host, {"op": "offer", "resource": {
                "url": f"http://127.0.0.1:{server.server_port}/fixture-{index}",
                "filename": filename, "resource_kind": "file", "client_request_id": uuid.uuid4().hex}})
            capture(presenter, "确认下载", f"confirm-{index}")
            accepted = request({"type": "command", "request_id": 1, "command": {
                "kind": "accept_handoff", "handoff_id": offered["handoff"]["id"],
                "filename": filename, "download_dir": str(local / "downloads"), "trusted_ui": True}})
            if accepted.get("type") == "error":
                raise AssertionError(accepted)
            deadline = time.monotonic() + 10
            while True:
                tasks = request({"type": "snapshot", "request_id": 2})["tasks"]
                created = next((t for t in tasks if t["filename"] == filename), None)
                if created:
                    break
                if time.monotonic() > deadline:
                    raise TimeoutError("Accepted handoff did not create a task")
                time.sleep(.05)
            request({"type": "command", "request_id": 3, "command": {
                "kind": "task_action", "task_id": created["task_id"], "action": "start"}})
            deadline = time.monotonic() + 10
            while True:
                task = next(t for t in request({"type": "snapshot", "request_id": 2})["tasks"]
                            if t["task_id"] == created["task_id"])
                if task["downloaded_bytes"] > 0:
                    break
                if task["status"] == "failed" or time.monotonic() > deadline:
                    raise AssertionError("Fixture did not transfer bytes")
                time.sleep(.02)
            capture(presenter, "下载进度", f"progress-{index}")
            capture(presenter, "下载完成", f"complete-{index}")
            deadline = time.monotonic() + 20
            while True:
                tasks = request({"type": "snapshot", "request_id": 2})["tasks"]
                task = next((t for t in tasks if t["filename"] == filename and t["status"] == "completed"), None)
                if task:
                    break
                if time.monotonic() > deadline:
                    raise TimeoutError("Download did not complete")
                time.sleep(.05)
            result = Path(task["output_path"])
            assert hashlib.sha256(result.read_bytes()).digest() == hashlib.sha256(b"repaint-fixture" * 65536).digest(), "Downloaded file changed"
            # Only this fixture's task/files; deleting it also hides its completion popup.
            deleted = request({"type": "command", "request_id": 3, "command": {
                "kind": "task_action", "task_id": task["task_id"], "action": "delete_files"}})
            assert deleted.get("type") != "error", deleted
            assert not result.exists(), "Fixture output remained after delete_files"
        report["passed"] = True
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
        server.shutdown()
        server.server_close()
        shutil.rmtree(local)
        (root / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf8")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    for name in ("presenter", "engine", "host", "output"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--dark", action="store_true")
    raise SystemExit(run(parser.parse_args()))
