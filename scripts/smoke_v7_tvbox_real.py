#!/usr/bin/env python3
"""Verify a real TVBox push and require the receiver to fetch the media."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse


def frame(message: dict[str, object]) -> bytes:
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    return struct.pack("<I", len(payload)) + payload


def read_exact(stream: socket.socket, size: int) -> bytes:
    result = bytearray()
    while len(result) < size:
        chunk = stream.recv(size - len(result))
        if not chunk:
            raise RuntimeError("Core IPC closed before the response completed")
        result.extend(chunk)
    return bytes(result)


def send_frame(stream: socket.socket, message: dict[str, object]) -> dict[str, object]:
    stream.sendall(frame(message))
    size = struct.unpack("<I", read_exact(stream, 4))[0]
    response = json.loads(read_exact(stream, size).decode("utf-8"))
    if response.get("type") == "error":
        raise RuntimeError(f"Core rejected request: {response}")
    return response


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def local_lan_ip(peer: str | None = None) -> str:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        probe.connect((peer or "192.168.255.255", 1))
        address = str(probe.getsockname()[0])
    octets = [int(part) for part in address.split(".")]
    private = (
        octets[0] == 10
        or (octets[0] == 172 and 16 <= octets[1] <= 31)
        or (octets[0] == 192 and octets[1] == 168)
    )
    if not private:
        raise RuntimeError(f"No private LAN address is available: {address}")
    return address


def wait_core(port: int, timeout: float = 15.0) -> socket.socket:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            stream = socket.create_connection(("127.0.0.1", port), timeout=0.3)
            stream.settimeout(20)
            hello = send_frame(
                stream,
                {"type": "hello", "protocol": "hls-downloader-v7-core", "version": 1},
            )
            if hello.get("type") == "hello":
                return stream
            stream.close()
        except OSError:
            time.sleep(0.1)
    raise TimeoutError("isolated v7 Core did not become ready")


def event_payload(response: dict[str, object], kind: str) -> dict[str, object] | None:
    for envelope in response.get("events", []):
        if not isinstance(envelope, dict):
            continue
        event = envelope.get("event")
        if isinstance(event, dict) and event.get("kind") == kind:
            return event
    return None


class MediaOrigin:
    def __init__(self, media: Path, bind_host: str, port: int) -> None:
        self.media = media
        self.requests: list[dict[str, object]] = []
        self.fetched = threading.Event()
        origin = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, _format: str, *_args: object) -> None:
                return

            def do_HEAD(self) -> None:  # noqa: N802
                self._serve(False)

            def do_GET(self) -> None:  # noqa: N802
                self._serve(True)

            def _serve(self, include_body: bool) -> None:
                if self.path.split("?", 1)[0] != "/tvbox-smoke.mp4":
                    self.send_error(404)
                    return
                payload = origin.media.read_bytes()
                start = 0
                end = len(payload) - 1
                range_header = self.headers.get("Range", "")
                if range_header.startswith("bytes="):
                    value = range_header[6:].split(",", 1)[0]
                    left, _, right = value.partition("-")
                    start = int(left or 0)
                    end = min(int(right) if right else end, end)
                if start < 0 or start > end or start >= len(payload):
                    self.send_response(416)
                    self.send_header("Content-Range", f"bytes */{len(payload)}")
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                body = payload[start : end + 1]
                status = 206 if range_header else 200
                self.send_response(status)
                self.send_header("Content-Type", "video/mp4")
                self.send_header("Accept-Ranges", "bytes")
                self.send_header("Content-Length", str(len(body)))
                if status == 206:
                    self.send_header("Content-Range", f"bytes {start}-{end}/{len(payload)}")
                self.send_header("Connection", "close")
                self.end_headers()
                if include_body:
                    self.wfile.write(body)
                    origin.fetched.set()
                origin.requests.append(
                    {
                        "client": self.client_address[0],
                        "method": self.command,
                        "path": self.path,
                        "range": range_header,
                        "status": status,
                        "bytes": len(body) if include_body else 0,
                    }
                )

        self.server = ThreadingHTTPServer((bind_host, port), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def port(self) -> int:
        return int(self.server.server_address[1])

    def start(self) -> None:
        self.thread.start()

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", required=True, type=Path)
    parser.add_argument("--media", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--expected-host")
    parser.add_argument("--fetch-timeout", type=float, default=20.0)
    args = parser.parse_args()

    if os.name != "nt":
        raise RuntimeError("real TVBox smoke requires Windows")
    if not args.engine.is_file() or not args.media.is_file():
        raise FileNotFoundError("engine or media fixture is missing")

    started = time.perf_counter()
    root = Path(tempfile.mkdtemp(prefix="hls-v7-tvbox-real-"))
    engine_path = root / "HLSDownloaderEngine.exe"
    shutil.copy2(args.engine, engine_path)
    core_port = free_port()
    lan_ip = local_lan_ip(args.expected_host)
    origin = MediaOrigin(args.media, "0.0.0.0", 0)
    origin.start()
    media_url = f"http://{lan_ip}:{origin.port}/tvbox-smoke.mp4"
    environment = os.environ.copy()
    environment.update(
        {
            "HLS_V7_DATA_DIR": str(root / "data"),
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
    stream: socket.socket | None = None
    report: dict[str, object] = {}
    try:
        stream = wait_core(core_port)
        discovered_at = time.perf_counter()
        discovery = send_frame(
            stream,
            {
                "type": "command",
                "request_id": 2,
                "command": {"kind": "discover_cast_devices", "mode": "tvbox"},
            },
        )
        devices_event = event_payload(discovery, "cast_devices") or {}
        devices = devices_event.get("devices", [])
        if not isinstance(devices, list):
            devices = []
        candidates = [item for item in devices if isinstance(item, dict)]
        if args.expected_host:
            candidates = [
                item
                for item in candidates
                if urlparse(str(item.get("location", ""))).hostname == args.expected_host
            ]
        if not candidates:
            raise RuntimeError(f"No matching TVBox receiver was discovered: {devices}")
        device = candidates[0]
        pushed_at = time.perf_counter()
        push = send_frame(
            stream,
            {
                "type": "command",
                "request_id": 3,
                "command": {
                    "kind": "share_media",
                    "path": "",
                    "url": media_url,
                    "title": "HLS Downloader v7 TVBox smoke",
                    "device_id": str(device["id"]),
                },
            },
        )
        cast_session = event_payload(push, "cast_session")
        if not cast_session or cast_session.get("device_kind") != "tvbox":
            raise RuntimeError(f"Core did not publish a TVBox session: {push}")
        fetched = origin.fetched.wait(args.fetch_timeout)
        fetch_deadline = time.perf_counter()
        if not fetched:
            raise RuntimeError(
                f"TVBox accepted the push but did not fetch {media_url}; requests={origin.requests}"
            )
        receiver_host = urlparse(str(device.get("location", ""))).hostname
        matching_requests = [
            item for item in origin.requests if item.get("client") == receiver_host
        ]
        if not matching_requests:
            raise RuntimeError(
                f"media was fetched, but not by the selected receiver {receiver_host}: {origin.requests}"
            )
        send_frame(
            stream,
            {
                "type": "command",
                "request_id": 4,
                "command": {"kind": "control_cast", "action": "stop"},
            },
        )
        report = {
            "schema": 1,
            "passed": True,
            "receiver": device,
            "media_url": media_url,
            "media_sha256": hashlib.sha256(args.media.read_bytes()).hexdigest().upper(),
            "discovery_ms": round((pushed_at - discovered_at) * 1000, 2),
            "push_to_fetch_ms": round((fetch_deadline - pushed_at) * 1000, 2),
            "requests": origin.requests,
            "cast_session": cast_session,
            "total_ms": round((fetch_deadline - started) * 1000, 2),
        }
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(report, ensure_ascii=False, separators=(",", ":")))
        return 0
    finally:
        if stream is not None:
            stream.close()
        origin.close()
        if engine.poll() is None:
            engine.terminate()
            try:
                engine.wait(timeout=3)
            except subprocess.TimeoutExpired:
                engine.kill()
                engine.wait(timeout=3)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
