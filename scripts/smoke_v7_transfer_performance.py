from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import time


PAYLOAD_SIZE = 96 * 1024 * 1024
PATTERN = bytes(range(256)) * 4096


class FixtureState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.bytes_sent = 0
        self.range_requests = 0
        self.full_requests = 0
        self.requested_ranges: list[tuple[int, int]] = []


class QuietThreadingHttpServer(ThreadingHTTPServer):
    def handle_error(self, _request: object, _client_address: object) -> None:
        return


class RangeHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    state: FixtureState

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def do_HEAD(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        self._headers(200, 0, PAYLOAD_SIZE - 1, PAYLOAD_SIZE)

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        start, end = 0, PAYLOAD_SIZE - 1
        range_header = self.headers.get("Range", "")
        status = 200
        if range_header.startswith("bytes="):
            status = 206
            first, _, last = range_header[6:].partition("-")
            start = int(first or "0")
            end = min(int(last) if last else PAYLOAD_SIZE - 1, PAYLOAD_SIZE - 1)
            with self.state.lock:
                self.state.range_requests += 1
                self.state.requested_ranges.append((start, end))
        else:
            with self.state.lock:
                self.state.full_requests += 1
        self._headers(status, start, end, PAYLOAD_SIZE)
        remaining = end - start + 1
        offset = start
        while remaining:
            block = min(remaining, 256 * 1024)
            chunk = bytes(PATTERN[(offset % len(PATTERN)) : (offset % len(PATTERN)) + block])
            if len(chunk) < block:
                chunk += PATTERN[: block - len(chunk)]
            try:
                self.wfile.write(chunk)
            except (BrokenPipeError, ConnectionResetError):
                return
            with self.state.lock:
                self.state.bytes_sent += len(chunk)
            remaining -= len(chunk)
            offset += len(chunk)

    def _headers(self, status: int, start: int, end: int, total: int) -> None:
        self.send_response(status)
        self.send_header("Accept-Ranges", "bytes")
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("ETag", '"v7-performance-fixture"')
        if status == 206:
            self.send_header("Content-Range", f"bytes {start}-{end}/{total}")
        self.end_headers()


def send_frame(stream: socket.socket, message: dict[str, object]) -> dict[str, object]:
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    stream.sendall(struct.pack("<I", len(payload)) + payload)
    header = read_exact(stream, 4)
    size = struct.unpack("<I", header)[0]
    return json.loads(read_exact(stream, size).decode("utf-8"))


def read_exact(stream: socket.socket, size: int) -> bytes:
    result = bytearray()
    while len(result) < size:
        chunk = stream.recv(size - len(result))
        if not chunk:
            raise RuntimeError("Core IPC closed before the response completed")
        result.extend(chunk)
    return bytes(result)


class ProcessMemoryCounters(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
    ]


def working_set_bytes(pid: int) -> int:
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    handle = kernel32.OpenProcess(0x0400 | 0x0010, False, pid)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        counters = ProcessMemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        if not psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return int(counters.WorkingSetSize)
    finally:
        kernel32.CloseHandle(handle)


def expected_sha256() -> str:
    digest = hashlib.sha256()
    remaining = PAYLOAD_SIZE
    while remaining:
        block = PATTERN[: min(remaining, len(PATTERN))]
        digest.update(block)
        remaining -= len(block)
    return digest.hexdigest()


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def wait_for_core(port: int, deadline: float) -> socket.socket:
    while time.monotonic() < deadline:
        try:
            stream = socket.create_connection(("127.0.0.1", port), timeout=0.2)
            stream.settimeout(5)
            hello = send_frame(
                stream,
                {"type": "hello", "protocol": "hls-downloader-v7-core", "version": 1},
            )
            if hello.get("type") == "hello":
                return stream
            stream.close()
        except OSError:
            time.sleep(0.02)
    raise TimeoutError("isolated Engine did not expose the Core IPC endpoint")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    if os.name != "nt":
        raise RuntimeError("this performance fixture measures a Windows Engine process")
    if not args.engine.is_file():
        raise FileNotFoundError(args.engine)

    root = Path(tempfile.mkdtemp(prefix="hls-v7-transfer-performance-"))
    engine_path = root / "HLSDownloaderEngine.exe"
    shutil.copy2(args.engine, engine_path)
    state = FixtureState()
    handler = type("BoundRangeHandler", (RangeHandler,), {"state": state})
    origin = QuietThreadingHttpServer(("127.0.0.1", 0), handler)
    origin_thread = threading.Thread(target=origin.serve_forever, daemon=True)
    origin_thread.start()
    core_port = free_port()
    environment = os.environ.copy()
    environment.update(
        {
            "HLS_V7_DATA_DIR": str(root / "data"),
            "HLS_V6_DATA_DIR": str(root / "data"),
            "HLS_V7_DOWNLOAD_DIR": str(root / "downloads"),
            "HLS_V7_CORE_TCP": "1",
            "HLS_V7_CORE_BIND": f"127.0.0.1:{core_port}",
            "HLS_V6_SKIP_LEGAL": "1",
            "HLS_V6_SKIP_MIGRATE": "1",
        }
    )
    engine = subprocess.Popen(
        [str(engine_path)],
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    try:
        ipc = wait_for_core(core_port, time.monotonic() + 10)
        baseline_memory = working_set_bytes(engine.pid)
        settings = send_frame(
            ipc,
            {
                "type": "store_setting",
                "request_id": 1,
                "key": "download_dir",
                "value": str(root / "downloads"),
            },
        )
        if settings.get("type") == "error":
            raise RuntimeError(f"Core rejected the isolated download root: {settings}")
        create = send_frame(
            ipc,
            {
                "type": "command",
                "request_id": 2,
                "command": {
                    "kind": "create_task",
                    "spec": {
                        "url": f"http://127.0.0.1:{origin.server_port}/fixture.bin",
                        "resource_kind": "file",
                        "title": "v7 transfer performance fixture",
                        "filename": "fixture.bin",
                        "concurrency": 8,
                        "expected_size": PAYLOAD_SIZE,
                    },
                },
            },
        )
        events = create.get("events", [])
        created = next(
            (item for item in events if item.get("event", {}).get("kind") == "task_created"),
            None,
        )
        if not created:
            raise RuntimeError(f"Core did not create the performance task: {create}")
        task_id = str(created["event"]["snapshot"]["task_id"])
        started = time.perf_counter()
        start = send_frame(
            ipc,
            {
                "type": "command",
                "request_id": 3,
                "command": {"kind": "task_action", "task_id": task_id, "action": "start"},
            },
        )
        if start.get("type") == "error":
            raise RuntimeError(f"Core rejected the performance task: {start}")
        peak_memory = baseline_memory
        final_snapshot: dict[str, object] | None = None
        request_id = 4
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            peak_memory = max(peak_memory, working_set_bytes(engine.pid))
            snapshot = send_frame(ipc, {"type": "snapshot", "request_id": request_id})
            request_id += 1
            task = next((item for item in snapshot.get("tasks", []) if item.get("task_id") == task_id), None)
            if task and task.get("status") in {"completed", "failed", "cancelled"}:
                final_snapshot = task
                break
            time.sleep(0.015)
        if not final_snapshot or final_snapshot.get("status") != "completed":
            raise RuntimeError(f"real transfer did not complete: {final_snapshot}")
        elapsed = time.perf_counter() - started
        output = Path(str(final_snapshot.get("output_path", "")))
        if not output.is_file() or output.stat().st_size != PAYLOAD_SIZE:
            raise RuntimeError("published output size does not match the fixture")
        digest = hashlib.sha256(output.read_bytes()).hexdigest()
        if digest != expected_sha256():
            raise RuntimeError("published output SHA-256 does not match the fixture")
        with state.lock:
            bytes_at_publish = state.bytes_sent
            range_requests = state.range_requests
            full_requests = state.full_requests
            requested_ranges = list(state.requested_ranges)
        time.sleep(0.3)
        with state.lock:
            bytes_after_settle = state.bytes_sent
        settled_memory = working_set_bytes(engine.pid)
        throughput_mib_s = PAYLOAD_SIZE / elapsed / (1024 * 1024)
        covered = bytearray(PAYLOAD_SIZE)
        requested_bytes = 0
        for range_start, range_end in requested_ranges:
            requested_bytes += range_end - range_start + 1
            covered[range_start : range_end + 1] = b"\x01" * (range_end - range_start + 1)
        unique_requested_bytes = covered.count(1)
        result = {
            "schema": 1,
            "payload_bytes": PAYLOAD_SIZE,
            "elapsed_ms": round(elapsed * 1000, 2),
            "throughput_mib_s": round(throughput_mib_s, 2),
            "sha256": digest,
            "range_requests": range_requests,
            "full_requests": full_requests,
            "requested_range_bytes": requested_bytes,
            "unique_requested_range_bytes": unique_requested_bytes,
            "overlapping_requested_range_bytes": requested_bytes - unique_requested_bytes,
            "bytes_sent_at_publish": bytes_at_publish,
            "post_publish_extra_network_bytes": bytes_after_settle - bytes_at_publish,
            "baseline_working_set_mib": round(baseline_memory / 1024 / 1024, 2),
            "peak_working_set_mib": round(peak_memory / 1024 / 1024, 2),
            "settled_working_set_mib": round(settled_memory / 1024 / 1024, 2),
            "working_set_growth_mib": round((peak_memory - baseline_memory) / 1024 / 1024, 2),
            "thresholds": {
                "minimum_local_throughput_mib_s": 20,
                "maximum_working_set_growth_mib": 256,
                "post_publish_extra_network_bytes": 0,
            },
        }
        result["passed"] = bool(
            throughput_mib_s >= 20
            and peak_memory - baseline_memory <= 256 * 1024 * 1024
            and bytes_after_settle == bytes_at_publish
            and range_requests > 0
        )
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False))
        if not result["passed"]:
            raise RuntimeError("real transfer performance thresholds failed")
        return 0
    finally:
        origin.shutdown()
        origin.server_close()
        if engine.poll() is None:
            engine.terminate()
            try:
                engine.wait(timeout=5)
            except subprocess.TimeoutExpired:
                engine.kill()
                engine.wait(timeout=5)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
