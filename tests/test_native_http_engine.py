from __future__ import annotations

import asyncio
import shutil
import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from backend.app.config import settings
from backend.app.downloader.http_file import HTTPDownloader
from backend.app.models import Task, TaskStatus, TaskType
from backend.app.native_engine import locate_native_engine_executable, write_native_job


BODY = bytes(range(256)) * 48


def test_locate_native_engine_prefers_explicit_file(tmp_path, monkeypatch):
    exe = tmp_path / "HLSNativeShell.exe"
    exe.write_bytes(b"not-a-real-binary")
    monkeypatch.setenv("HLS_NATIVE_ENGINE", str(exe))
    assert locate_native_engine_executable() == exe


def test_write_native_job_roundtrip(tmp_path):
    path = tmp_path / "job.json"
    write_native_job(
        job_path=path,
        payload={
            "url": "http://127.0.0.1/file.bin",
            "output": str(tmp_path / "payload.downloading"),
            "connections": 4,
            "sequential": False,
            "control": str(tmp_path / "control"),
            "progress": str(tmp_path / "progress.json"),
        },
    )
    text = path.read_text(encoding="utf-8")
    assert "payload.downloading" in text
    assert "127.0.0.1" in text


def test_http_run_does_not_use_native_engine_under_pytest(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    monkeypatch.setattr(settings, "native_http_engine", True)
    task = Task(id="native-off", url="https://files.test/a.bin", task_type=TaskType.HTTP)
    downloader = HTTPDownloader(task)
    assert downloader._native_http_engine_eligible(tmp_path / "http-resume.json") is False


def _range_server(body: bytes) -> tuple[ThreadingHTTPServer, str]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_args):
            return

        def do_GET(self):
            range_header = self.headers.get("Range", "")
            if range_header.startswith("bytes="):
                spec = range_header.split("=", 1)[1]
                start_text, end_text = spec.split("-", 1)
                start = int(start_text or "0")
                end = int(end_text) if end_text else len(body) - 1
                end = min(end, len(body) - 1)
                slice_body = body[start : end + 1]
                self.send_response(206)
                self.send_header("Content-Range", f"bytes {start}-{end}/{len(body)}")
                self.send_header("Content-Length", str(len(slice_body)))
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Accept-Ranges", "bytes")
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(slice_body)
                return
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)

    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, f"http://127.0.0.1:{port}/file.bin"


@pytest.mark.skipif(shutil.which("cargo") is None, reason="cargo is required to build the native HTTP engine")
def test_native_engine_range_download_writes_one_file(tmp_path, monkeypatch):
    from backend.app.native_engine import build_native_engine_debug

    executable = build_native_engine_debug()
    if executable is None:
        pytest.skip("native HTTP engine failed to build")
    monkeypatch.setenv("HLS_TEST_NATIVE_HTTP", "1")
    monkeypatch.setenv("HLS_NATIVE_ENGINE", str(executable))
    monkeypatch.setattr(settings, "native_http_engine", True)
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    monkeypatch.setattr(settings, "proxy_mode", "direct")
    server, url = _range_server(BODY)
    try:
        task = Task(
            id="native-range",
            url=url,
            task_type=TaskType.HTTP,
            filename="payload.bin",
            concurrency=4,
        )
        asyncio.run(HTTPDownloader(task).run())
    finally:
        server.shutdown()
    assert task.status is TaskStatus.DONE
    assert task.engine_state.get("http_engine") == "native-shell"
    output = Path(task.output_path)
    assert output.read_bytes() == BODY
