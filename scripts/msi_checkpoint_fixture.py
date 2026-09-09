from __future__ import annotations

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import threading
import time


PAYLOAD_SIZE = 32 * 1024 * 1024
BLOCK = bytes(range(256)) * 128


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def handle_error(self, _request: object, _address: object) -> None:
        pass


class RangeHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format: str, *_args: object) -> None:
        pass

    def do_HEAD(self) -> None:  # noqa: N802
        self._headers(200, 0, PAYLOAD_SIZE - 1)

    def do_GET(self) -> None:  # noqa: N802
        start, end, status = 0, PAYLOAD_SIZE - 1, 200
        value = self.headers.get("Range", "")
        if value.startswith("bytes="):
            first, _, last = value[6:].partition("-")
            start = int(first or 0)
            end = min(int(last) if last else PAYLOAD_SIZE - 1, PAYLOAD_SIZE - 1)
            status = 206
        self._headers(status, start, end)
        remaining = end - start + 1
        offset = start
        while remaining:
            size = min(remaining, len(BLOCK))
            pos = offset % len(BLOCK)
            chunk = (BLOCK[pos:] + BLOCK[:pos])[:size]
            try:
                self.wfile.write(chunk)
                self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError):
                return
            remaining -= size
            offset += size
            time.sleep(0.01)

    def _headers(self, status: int, start: int, end: int) -> None:
        self.send_response(status)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("ETag", '"v7-msi-checkpoint"')
        if status == 206:
            self.send_header("Content-Range", f"bytes {start}-{end}/{PAYLOAD_SIZE}")
        self.end_headers()


def read_exact(stream: socket.socket, size: int) -> bytes:
    data = bytearray()
    while len(data) < size:
        chunk = stream.recv(size - len(data))
        if not chunk:
            raise RuntimeError("Core IPC closed")
        data.extend(chunk)
    return bytes(data)


def send(stream: socket.socket, message: dict[str, object]) -> dict[str, object]:
    payload = json.dumps(message, separators=(",", ":")).encode()
    stream.sendall(struct.pack("<I", len(payload)) + payload)
    return json.loads(read_exact(stream, struct.unpack("<I", read_exact(stream, 4))[0]))


def connect(port: int) -> socket.socket:
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            stream = socket.create_connection(("127.0.0.1", port), timeout=1)
            stream.settimeout(10)
            hello = send(stream, {"type": "hello", "protocol": "hls-downloader-v7-core", "version": 1})
            if hello.get("type") == "hello":
                return stream
        except OSError:
            time.sleep(0.05)
    raise TimeoutError("Engine TCP IPC did not become ready")


def snapshot(stream: socket.socket, task_id: str, request_id: int) -> dict[str, object]:
    response = send(stream, {"type": "snapshot", "request_id": request_id})
    task = next((item for item in response.get("tasks", []) if item.get("task_id") == task_id), None)
    if not task:
        raise RuntimeError(f"task is absent from snapshot: {response}")
    return task


def wait_task(stream: socket.socket, task_id: str, predicate, request_id: int) -> dict[str, object]:
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        task = snapshot(stream, task_id, request_id)
        request_id += 1
        if predicate(task):
            return task
        time.sleep(0.05)
    raise TimeoutError(f"task state deadline expired: {task}")


def file_hash(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", required=True, choices=("create-pause", "verify-resume", "verify-paused"))
    parser.add_argument("--engine", required=True, type=Path)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--state", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--core-port", required=True, type=int)
    parser.add_argument("--origin-port", required=True, type=int)
    args = parser.parse_args()
    args.root.mkdir(parents=True, exist_ok=True)
    data = args.root / "data"
    downloads = args.root / "downloads"
    data.mkdir(exist_ok=True)
    downloads.mkdir(exist_ok=True)
    origin = Server(("127.0.0.1", args.origin_port), RangeHandler)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    env = os.environ.copy()
    env.update({
        "HLS_V7_DATA_DIR": str(data),
        "HLS_V7_DOWNLOAD_DIR": str(downloads),
        "HLS_V7_CORE_TCP": "1",
        "HLS_V7_CORE_BIND": f"127.0.0.1:{args.core_port}",
        "HLS_V6_SKIP_MIGRATE": "1",
    })
    engine = subprocess.Popen([str(args.engine)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        ipc = connect(args.core_port)
        if args.mode == "create-pause":
            send(ipc, {"type": "store_setting", "request_id": 1, "key": "download_dir", "value": str(downloads)})
            created = send(ipc, {"type": "command", "request_id": 2, "command": {"kind": "create_task", "spec": {
                "url": f"http://127.0.0.1:{args.origin_port}/fixture.bin", "resource_kind": "file",
                "title": "MSI checkpoint fixture", "filename": "fixture.bin", "concurrency": 1,
                "expected_size": PAYLOAD_SIZE,
            }}})
            event = next((item for item in created.get("events", []) if item.get("event", {}).get("kind") == "task_created"), None)
            if not event:
                raise RuntimeError(f"create_task failed: {created}")
            task_id = str(event["event"]["snapshot"]["task_id"])
            send(ipc, {"type": "command", "request_id": 3, "command": {"kind": "task_action", "task_id": task_id, "action": "start"}})
            wait_task(ipc, task_id, lambda item: int(item.get("downloaded_bytes", 0)) >= 256 * 1024, 10)
            send(ipc, {"type": "command", "request_id": 100, "command": {"kind": "task_action", "task_id": task_id, "action": "pause"}})
            task = wait_task(ipc, task_id, lambda item: item.get("status") == "paused" and int(item.get("active_workers", -1)) == 0, 110)
            args.state.write_text(json.dumps({"task_id": task_id, "downloaded_bytes": int(task["downloaded_bytes"])}, indent=2), encoding="utf-8")
        else:
            state = json.loads(args.state.read_text(encoding="utf-8"))
            task_id = state["task_id"]
            task = snapshot(ipc, task_id, 200)
            if task.get("status") != "paused" or int(task.get("downloaded_bytes", 0)) < int(state["downloaded_bytes"]):
                raise RuntimeError(f"persisted task regressed: {task}")
            if args.mode == "verify-resume":
                send(ipc, {"type": "command", "request_id": 201, "command": {"kind": "task_action", "task_id": task_id, "action": "resume"}})
                task = wait_task(ipc, task_id, lambda item: item.get("status") in {"completed", "failed"}, 210)
                if task.get("status") != "completed":
                    raise RuntimeError(f"resumed task did not complete: {task}")
        engine.terminate()
        engine.wait(timeout=10)
        database = data / "data.db"
        if not database.is_file():
            raise RuntimeError(f"Core database is missing: {database}")
        report = {"schema": 1, "mode": args.mode, "task_id": task_id, "status": task["status"],
                  "downloaded_bytes": int(task["downloaded_bytes"]), "active_workers": int(task["active_workers"]),
                  "data_db": str(database), "data_db_sha256": file_hash(database), "engine_pid": engine.pid}
        args.report.write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(json.dumps(report, separators=(",", ":")))
        return 0
    finally:
        origin.shutdown()
        if engine.poll() is None:
            engine.terminate()
            try:
                engine.wait(timeout=10)
            except subprocess.TimeoutExpired:
                engine.kill()


if __name__ == "__main__":
    raise SystemExit(main())
