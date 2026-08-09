"""End-to-end download smoke test for a packaged portable release or source tree.

The test runs only against a private loopback origin and an extracted copy of
the portable archive. It exercises the real packaged Core, HTTP pause/resume,
HLS download, incremental playback and cleanup without touching user state.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlsplit
from urllib.request import Request, urlopen
import zipfile


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def api_request(
    base: str,
    path: str,
    *,
    token: str = "",
    method: str = "GET",
    body: dict | None = None,
    timeout: float = 10,
) -> tuple[int, object]:
    data = None if body is None else json.dumps(body).encode("utf-8")
    headers = {"Content-Type": "application/json"}
    if token:
        headers["X-Token"] = token
    request = Request(base + path, data=data, headers=headers, method=method)
    try:
        with urlopen(request, timeout=timeout) as response:
            raw = response.read()
            return response.status, json.loads(raw) if raw else {}
    except HTTPError as exc:
        raw = exc.read()
        try:
            payload: object = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            payload = raw.decode("utf-8", errors="replace")
        return exc.code, payload


def wait_task(base: str, token: str, task_id: str, predicate, timeout: float = 45) -> dict:
    deadline = time.monotonic() + timeout
    latest: dict = {}
    while time.monotonic() < deadline:
        status, payload = api_request(base, "/api/tasks", token=token)
        if status == 200 and isinstance(payload, list):
            latest = next((item for item in payload if item.get("id") == task_id), latest)
            if latest and predicate(latest):
                return latest
        time.sleep(0.08)
    raise RuntimeError(f"task {task_id} did not reach expected state; last={latest.get('status', 'missing')}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


class SmokeOrigin(BaseHTTPRequestHandler):
    root: Path
    _stats_lock = threading.Lock()
    _active_ranges = 0
    _max_active_ranges = 0
    _range_requests = 0
    _range_spans: set[tuple[int, int]] = set()
    _llhls_polls = 0

    @classmethod
    def reset_range_stats(cls) -> None:
        with cls._stats_lock:
            cls._active_ranges = 0
            cls._max_active_ranges = 0
            cls._range_requests = 0
            cls._range_spans = set()

    @classmethod
    def reset_llhls(cls) -> None:
        with cls._stats_lock:
            cls._llhls_polls = 0

    @classmethod
    def range_stats(cls) -> dict[str, int]:
        with cls._stats_lock:
            return {
                "max_active": cls._max_active_ranges,
                "requests": cls._range_requests,
                "distinct_spans": len(cls._range_spans),
            }

    def log_message(self, _format: str, *_args) -> None:
        return

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        parsed_request = urlsplit(self.path)
        relative = parsed_request.path.lstrip("/")
        if not relative or ".." in Path(relative).parts:
            self.send_error(404)
            return
        path = (self.root / relative).resolve()
        if not path.is_file() or self.root.resolve() not in path.parents:
            self.send_error(404)
            return
        if relative == "llhls/index.m3u8":
            with type(self)._stats_lock:
                type(self)._llhls_polls += 1
                poll = type(self)._llhls_polls
            head = (
                "#EXTM3U\n#EXT-X-VERSION:9\n#EXT-X-TARGETDURATION:2\n"
                "#EXT-X-PART-INF:PART-TARGET=0.333\n"
                "#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=1.0\n"
                "#EXT-X-MEDIA-SEQUENCE:0\n"
                '#EXT-X-MAP:URI="init.mp4"\n'
                '#EXT-X-PART:DURATION=2.0,URI="segment-000.m4s",INDEPENDENT=YES\n'
            )
            body = head + (
                '#EXT-X-PRELOAD-HINT:TYPE=PART,URI="future.m4s"\n'
                if poll == 1
                else (
                    "#EXTINF:2.0,\nsegment-000.m4s\n"
                    "#EXTINF:2.0,\nsegment-001.m4s\n#EXT-X-ENDLIST\n"
                )
            )
            payload = body.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/vnd.apple.mpegurl")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        if path.name == "signed.bin" and parse_qs(parsed_request.query).get("s") != ["fresh"]:
            self.send_error(403)
            return
        size = path.stat().st_size
        start, end = 0, max(0, size - 1)
        partial = False
        range_header = self.headers.get("Range", "")
        supports_range = path.name != "norange.bin"
        if supports_range and range_header.startswith("bytes="):
            try:
                raw_start, raw_end = range_header[6:].split("-", 1)
                start = int(raw_start or 0)
                end = min(end, int(raw_end)) if raw_end else end
                partial = True
            except (TypeError, ValueError):
                self.send_error(416)
                return
        if start < 0 or end < start or start >= size:
            self.send_response(416)
            self.send_header("Content-Range", f"bytes */{size}")
            self.end_headers()
            return
        if path.suffix.lower() == ".ts":
            # Keep the HLS task alive long enough to open incremental playback.
            time.sleep(1.1)
        # Exclude the 256-byte metadata probe. Only worker-owned byte ranges
        # count toward the parallelism proof.
        tracks_worker_range = (
            path.name == "range.bin" and partial and end - start + 1 > 256
        )
        if tracks_worker_range:
            with type(self)._stats_lock:
                type(self)._active_ranges += 1
                type(self)._range_requests += 1
                type(self)._range_spans.add((start, end))
                type(self)._max_active_ranges = max(
                    type(self)._max_active_ranges,
                    type(self)._active_ranges,
                )

        self.send_response(206 if partial else 200)
        self.send_header("Content-Type", "application/octet-stream")
        if supports_range:
            self.send_header("Accept-Ranges", "bytes")
        self.send_header("ETag", '"hls-downloader-smoke-v1"')
        self.send_header("Last-Modified", "Wed, 29 Jul 2026 00:00:00 GMT")
        self.send_header("Content-Length", str(end - start + 1))
        if partial:
            self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
        self.end_headers()
        try:
            with path.open("rb") as stream:
                stream.seek(start)
                remaining = end - start + 1
                while remaining:
                    block = stream.read(min(64 * 1024, remaining))
                    if not block:
                        break
                    self.wfile.write(block)
                    self.wfile.flush()
                    remaining -= len(block)
                    if path.name == "range.bin":
                        time.sleep(0.012)
        except (BrokenPipeError, ConnectionAbortedError, ConnectionResetError):
            return
        finally:
            if tracks_worker_range:
                with type(self)._stats_lock:
                    type(self)._active_ranges = max(0, type(self)._active_ranges - 1)


def generate_hls(ffmpeg: Path, origin: Path) -> None:
    hls_dir = origin / "hls"
    hls_dir.mkdir(parents=True)
    command = [
        str(ffmpeg), "-hide_banner", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=24",
        "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000",
        "-t", "12", "-c:v", "libx264", "-preset", "ultrafast",
        "-g", "48", "-sc_threshold", "0", "-c:a", "aac",
        "-f", "hls", "-hls_time", "2", "-hls_list_size", "0",
        "-hls_segment_filename", str(hls_dir / "segment-%03d.ts"),
        str(hls_dir / "index.m3u8"),
    ]
    completed = subprocess.run(command, capture_output=True, text=True, timeout=90)
    if completed.returncode:
        raise RuntimeError(f"ffmpeg HLS fixture generation failed: {completed.stderr[-500:]}")

    llhls_dir = origin / "llhls"
    llhls_dir.mkdir(parents=True)
    llhls_command = [
        str(ffmpeg), "-hide_banner", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=24",
        "-f", "lavfi", "-i", "sine=frequency=990:sample_rate=48000",
        "-t", "4", "-c:v", "libx264", "-preset", "ultrafast",
        "-g", "48", "-sc_threshold", "0", "-c:a", "aac",
        "-f", "hls", "-hls_time", "2", "-hls_list_size", "0",
        "-hls_segment_type", "fmp4", "-hls_fmp4_init_filename", "init.mp4",
        "-hls_segment_filename", str(llhls_dir / "segment-%03d.m4s"),
        str(llhls_dir / "index.m3u8"),
    ]
    completed = subprocess.run(
        llhls_command,
        cwd=llhls_dir,
        capture_output=True,
        text=True,
        timeout=90,
    )
    if completed.returncode:
        raise RuntimeError(f"ffmpeg LL-HLS fixture generation failed: {completed.stderr[-500:]}")


def generate_dash(ffmpeg: Path, origin: Path) -> None:
    dash_dir = origin / "dash"
    dash_dir.mkdir(parents=True)
    command = [
        str(ffmpeg), "-hide_banner", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=24",
        "-f", "lavfi", "-i", "sine=frequency=660:sample_rate=48000",
        "-t", "12", "-c:v", "libx264", "-preset", "ultrafast",
        "-g", "48", "-sc_threshold", "0", "-c:a", "aac",
        "-f", "dash", "-seg_duration", "2", "-use_template", "1", "-use_timeline", "1",
        "manifest.mpd",
    ]
    completed = subprocess.run(command, cwd=dash_dir, capture_output=True, text=True, timeout=90)
    if completed.returncode:
        raise RuntimeError(f"ffmpeg DASH fixture generation failed: {completed.stderr[-500:]}")


def run(archive: Path | None = None) -> dict:
    root = Path(__file__).resolve().parent.parent
    smoke_root = (root / "build" / "real-download-smoke").resolve()
    build_root = (root / "build").resolve()
    if build_root not in smoke_root.parents:
        raise RuntimeError("unsafe smoke cleanup path")
    shutil.rmtree(smoke_root, ignore_errors=True)
    portable = smoke_root / "portable"
    origin = smoke_root / "origin"
    portable.mkdir(parents=True)
    origin.mkdir(parents=True)
    core: subprocess.Popen | None = None
    core_log_stream = None
    completed = False
    server: ThreadingHTTPServer | None = None
    server_thread: threading.Thread | None = None
    try:
        if archive is not None:
            with zipfile.ZipFile(archive) as package:
                package.extractall(portable)
            core_exe = portable / "HLSDownloaderCore.exe"
            ffmpeg = portable / "bin" / "ffmpeg.exe"
            if not core_exe.is_file() or not ffmpeg.is_file() or not (portable / "portable").is_file():
                raise RuntimeError("portable archive is missing Core, FFmpeg or portable marker")
            core_command = [str(core_exe)]
            runtime_kind = "packaged"
        else:
            # Source smoke must not touch the developer's config, database,
            # task folders or downloads. A copied backend resolves this folder
            # as PROJECT_ROOT while using the same working-tree source code.
            shutil.copytree(
                root / "backend",
                portable / "backend",
                dirs_exist_ok=True,
                ignore=shutil.ignore_patterns("__pycache__", "*.pyc", "data.db", "data.db-*", "dist"),
            )
            # Legal documents are resolved relative to the isolated runtime
            # root, so copy them alongside the source Core for this smoke run.
            for legal_name in ("TERMS.md", "PRIVACY.md"):
                shutil.copy2(root / legal_name, portable / legal_name)
            ffmpeg_command = shutil.which("ffmpeg")
            if not ffmpeg_command:
                raise RuntimeError("ffmpeg is not available on PATH")
            ffmpeg = Path(ffmpeg_command).resolve()
            core_command = [os.environ.get("PYTHON", os.sys.executable), str(portable / "backend" / "run_core.py")]
            runtime_kind = "source"

        range_file = origin / "range.bin"
        pattern = hashlib.sha256(b"hls-downloader-real-download-smoke").digest()
        megabyte = pattern * ((1024 * 1024) // len(pattern))
        with range_file.open("wb") as stream:
            for _ in range(24):
                stream.write(megabyte)
        expected_range_hash = sha256(range_file)
        no_range_file = origin / "norange.bin"
        with no_range_file.open("wb") as stream:
            for _ in range(6):
                stream.write(megabyte)
        expected_no_range_hash = sha256(no_range_file)
        signed_file = origin / "signed.bin"
        with signed_file.open("wb") as stream:
            for _ in range(4):
                stream.write(megabyte)
        expected_signed_hash = sha256(signed_file)
        generate_hls(ffmpeg, origin)
        generate_dash(ffmpeg, origin)

        origin_port = free_port()
        core_port = free_port()
        SmokeOrigin.root = origin
        server = ThreadingHTTPServer(("127.0.0.1", origin_port), SmokeOrigin)
        server_thread = threading.Thread(target=server.serve_forever, daemon=True)
        server_thread.start()

        token = hashlib.sha256(os.urandom(32)).hexdigest()
        config = {
            "config_version": 18,
            "host": "127.0.0.1",
            "port": core_port,
            "token": token,
            "download_dir": "downloads",
            "temp_dir": ".",
            "ffmpeg_path": str(ffmpeg),
            "default_concurrency": 2,
            "max_concurrent_tasks": 1,
            "http_chunk_size_mb": 1,
            "proxy_mode": "direct",
        }
        (portable / "config.json").write_text(json.dumps(config), encoding="utf-8")
        core_log_stream = (smoke_root / "core.log").open("wb")
        core = subprocess.Popen(
            core_command, cwd=portable,
            stdout=core_log_stream, stderr=subprocess.STDOUT,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        base = f"http://127.0.0.1:{core_port}"
        health: dict = {}
        deadline = time.monotonic() + 35
        while time.monotonic() < deadline:
            try:
                status, payload = api_request(base, "/api/health", timeout=1)
                if status == 200 and isinstance(payload, dict):
                    health = payload
                    break
            except OSError:
                pass
            if core.poll() is not None:
                raise RuntimeError(f"packaged Core exited early with {core.returncode}")
            time.sleep(0.15)
        if not health:
            raise RuntimeError("packaged Core did not become healthy")

        # Task creation is intentionally gated by the first-use legal
        # acceptance.  The smoke harness runs against an isolated temporary
        # config, so accept the exact document digest returned by the Core
        # rather than bypassing the production gate or hard-coding a digest.
        legal_status, legal_terms = api_request(base, "/api/legal/terms", token=token)
        if legal_status != 200 or not isinstance(legal_terms, dict):
            raise RuntimeError(f"legal terms read failed: {legal_status}")
        legal_accept_status, _ = api_request(
            base,
            "/api/legal/accept",
            token=token,
            method="POST",
            body={
                "version": legal_terms.get("required_version", ""),
                "document_digest": legal_terms.get("document_digest", ""),
                "accepted": True,
            },
        )
        if legal_accept_status != 200:
            raise RuntimeError(f"legal terms acceptance failed: {legal_accept_status}")

        file_url = f"http://127.0.0.1:{origin_port}/range.bin?signature=smoke&expires=4102444800"
        SmokeOrigin.reset_range_stats()
        status, created = api_request(base, "/api/tasks", token=token, method="POST", body={
            "url": file_url, "task_type": "http", "filename": "range-smoke.bin",
            "concurrency": 2,
        })
        if status != 200 or not isinstance(created, dict):
            raise RuntimeError(f"HTTP task creation failed: {status}")
        http_id = str(created["id"])
        wait_task(base, token, http_id, lambda item: item.get("downloaded_bytes", 0) > 0 and item.get("status") == "downloading")
        pause_status, _ = api_request(base, f"/api/tasks/{http_id}/pause", token=token, method="POST")
        if pause_status != 200:
            raise RuntimeError(f"HTTP pause failed: {pause_status}")
        wait_task(base, token, http_id, lambda item: item.get("status") == "paused")
        resume_status, _ = api_request(base, f"/api/tasks/{http_id}/resume", token=token, method="POST")
        if resume_status != 200:
            raise RuntimeError(f"HTTP resume failed: {resume_status}")
        http_task = wait_task(base, token, http_id, lambda item: item.get("status") in {"done", "failed"}, timeout=90)
        if http_task.get("status") != "done":
            raise RuntimeError(f"HTTP download failed: {http_task.get('error_code') or http_task.get('last_log')}")
        http_output = Path(str(http_task["output_path"]))
        if sha256(http_output) != expected_range_hash:
            raise RuntimeError("HTTP pause/resume output checksum mismatch")
        http_output_size = http_output.stat().st_size
        range_stats = SmokeOrigin.range_stats()
        if range_stats["max_active"] < 2:
            raise RuntimeError(
                "HTTP configured concurrency did not produce two simultaneous "
                f"worker Range requests: {range_stats}"
            )
        if range_stats["requests"] < 2 or range_stats["distinct_spans"] < 2:
            raise RuntimeError(f"HTTP worker pool did not request distinct ranges: {range_stats}")

        no_range_url = f"http://127.0.0.1:{origin_port}/norange.bin?signature=smoke"
        status, created = api_request(base, "/api/tasks", token=token, method="POST", body={
            "url": no_range_url, "task_type": "http", "filename": "norange-smoke.bin",
            "concurrency": 4,
        })
        if status != 200 or not isinstance(created, dict):
            raise RuntimeError(f"no-Range HTTP task creation failed: {status}")
        no_range_id = str(created["id"])
        no_range_task = wait_task(base, token, no_range_id, lambda item: item.get("status") in {"done", "failed"}, timeout=90)
        if no_range_task.get("status") != "done":
            raise RuntimeError(f"no-Range HTTP download failed: {no_range_task.get('error_code') or no_range_task.get('last_log')}")
        no_range_output = Path(str(no_range_task["output_path"]))
        if sha256(no_range_output) != expected_no_range_hash:
            raise RuntimeError("no-Range HTTP output checksum mismatch")
        no_range_output_size = no_range_output.stat().st_size

        stale_signed_url = f"http://127.0.0.1:{origin_port}/signed.bin?s=stale&e=4102444800&_t=1"
        status, created = api_request(base, "/api/tasks", token=token, method="POST", body={
            "url": stale_signed_url, "task_type": "http", "filename": "signed-smoke.bin",
            "concurrency": 2,
        })
        if status != 200 or not isinstance(created, dict):
            raise RuntimeError(f"signed HTTP task creation failed: {status}")
        signed_id = str(created["id"])
        stale_task = wait_task(base, token, signed_id, lambda item: item.get("status") in {"failed", "done"}, timeout=45)
        if stale_task.get("status") != "failed" or int(stale_task.get("http_status") or 0) != 403:
            raise RuntimeError("stale signed HTTP request did not fail with HTTP 403")
        fresh_signed_url = f"http://127.0.0.1:{origin_port}/signed.bin?s=fresh&e=4102444800&_t=2"
        refresh_status, _ = api_request(
            base,
            f"/api/tasks/{signed_id}/request",
            token=token,
            method="PATCH",
            body={"url": fresh_signed_url, "auto_resume": True},
        )
        if refresh_status != 200:
            raise RuntimeError(f"signed HTTP request refresh failed: {refresh_status}")
        signed_task = wait_task(base, token, signed_id, lambda item: item.get("status") in {"done", "failed"}, timeout=90)
        if signed_task.get("status") != "done":
            raise RuntimeError(f"refreshed signed HTTP download failed: {signed_task.get('error_code') or signed_task.get('last_log')}")
        signed_output = Path(str(signed_task["output_path"]))
        if sha256(signed_output) != expected_signed_hash:
            raise RuntimeError("refreshed signed HTTP output checksum mismatch")
        signed_output_size = signed_output.stat().st_size

        hls_url = f"http://127.0.0.1:{origin_port}/hls/index.m3u8?token=current"
        status, created = api_request(base, "/api/tasks", token=token, method="POST", body={
            "url": hls_url, "task_type": "hls", "filename": "hls-smoke.mp4",
            "concurrency": 2,
        })
        if status != 200 or not isinstance(created, dict):
            raise RuntimeError(f"HLS task creation failed: {status}")
        hls_id = str(created["id"])
        session: dict | None = None
        playlist_text = ""
        playback_started_at = time.monotonic()
        playback_deadline = playback_started_at + 45
        playback_open_latency = 0.0
        first_local_ready_seconds = 0.0
        while time.monotonic() < playback_deadline:
            request_started = time.monotonic()
            open_status, opened = api_request(base, f"/api/tasks/{hls_id}/playback", token=token, method="POST")
            if open_status == 200 and isinstance(opened, dict):
                playback_open_latency = time.monotonic() - request_started
                first_local_ready_seconds = time.monotonic() - playback_started_at
                session = opened
                break
            time.sleep(0.1)
        if not session:
            raise RuntimeError("incremental HLS playback never became ready")
        session_id = str(session["session_id"])
        playback_token = str(session["playback_token"])
        query = urlencode({"session": session_id, "token": playback_token})
        request = Request(f"{base}/api/tasks/{hls_id}/playback/index.m3u8?{query}")
        playlist_started = time.monotonic()
        with urlopen(request, timeout=5) as response:
            playlist_text = response.read().decode("utf-8")
        playlist_latency = time.monotonic() - playlist_started
        if playback_open_latency > 2 or playlist_latency > 2:
            raise RuntimeError(
                f"local playback startup is too slow: open={playback_open_latency:.2f}s playlist={playlist_latency:.2f}s"
            )
        if "full=1" in playlist_text or "segments/000000.seg" not in playlist_text:
            raise RuntimeError("initial playback playlist is not the contiguous local prefix")
        segment_url = next(line for line in playlist_text.splitlines() if line.startswith("segments/"))
        with urlopen(f"{base}/api/tasks/{hls_id}/playback/{segment_url}", timeout=5) as response:
            if response.status != 200 or not response.read(32):
                raise RuntimeError("local playback segment could not be read")
        api_request(base, f"/api/tasks/{hls_id}/playback?session={session_id}", token=token, method="DELETE")

        hls_task = wait_task(base, token, hls_id, lambda item: item.get("status") in {"done", "failed"}, timeout=120)
        if hls_task.get("status") != "done":
            raise RuntimeError(f"HLS download failed: {hls_task.get('error_code') or hls_task.get('last_log')}")
        hls_output = Path(str(hls_task["output_path"]))
        if not hls_output.is_file() or hls_output.stat().st_size < 50_000:
            raise RuntimeError("HLS output is missing or unexpectedly small")
        probe = subprocess.run(
            [str(ffmpeg.parent / "ffprobe.exe"), "-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1", str(hls_output)],
            capture_output=True, text=True, timeout=20,
        )
        if probe.returncode or float(probe.stdout.strip() or 0) < 10:
            raise RuntimeError("HLS output failed ffprobe duration validation")
        hls_output_size = hls_output.stat().st_size

        SmokeOrigin.reset_llhls()
        llhls_url = f"http://127.0.0.1:{origin_port}/llhls/index.m3u8?token=current"
        status, created = api_request(base, "/api/tasks", token=token, method="POST", body={
            "url": llhls_url, "task_type": "hls", "filename": "llhls-smoke.mp4",
            "concurrency": 2,
        })
        if status != 200 or not isinstance(created, dict):
            raise RuntimeError(f"LL-HLS task creation failed: {status}")
        llhls_id = str(created["id"])
        llhls_task = wait_task(base, token, llhls_id, lambda item: item.get("status") in {"done", "failed"}, timeout=120)
        if llhls_task.get("status") != "done":
            raise RuntimeError(f"LL-HLS PART-only recording failed: {llhls_task.get('error_code') or llhls_task.get('last_log')}")
        llhls_output = Path(str(llhls_task["output_path"]))
        llhls_probe = subprocess.run(
            [str(ffmpeg.parent / "ffprobe.exe"), "-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1", str(llhls_output)],
            capture_output=True, text=True, timeout=20,
        )
        llhls_duration = float(llhls_probe.stdout.strip() or 0)
        if llhls_probe.returncode or not 3.5 <= llhls_duration <= 4.5:
            raise RuntimeError(f"LL-HLS output duration is invalid: {llhls_duration}")
        llhls_output_size = llhls_output.stat().st_size

        dash_url = f"http://127.0.0.1:{origin_port}/dash/manifest.mpd?token=current"
        status, created = api_request(base, "/api/tasks", token=token, method="POST", body={
            "url": dash_url, "task_type": "dash", "filename": "dash-smoke.mp4",
            "concurrency": 2,
        })
        if status != 200 or not isinstance(created, dict):
            raise RuntimeError(f"DASH task creation failed: {status}")
        dash_id = str(created["id"])
        dash_task = wait_task(base, token, dash_id, lambda item: item.get("status") in {"done", "failed"}, timeout=120)
        if dash_task.get("status") != "done":
            raise RuntimeError(f"DASH download failed: {dash_task.get('error_code') or dash_task.get('last_log')}")
        dash_output = Path(str(dash_task["output_path"]))
        dash_probe = subprocess.run(
            [str(ffmpeg.parent / "ffprobe.exe"), "-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1", str(dash_output)],
            capture_output=True, text=True, timeout=20,
        )
        if dash_probe.returncode or float(dash_probe.stdout.strip() or 0) < 10:
            raise RuntimeError("DASH output failed ffprobe duration validation")
        dash_output_size = dash_output.stat().st_size

        smoke_task_ids = (http_id, no_range_id, signed_id, hls_id, llhls_id, dash_id)
        for task_id in smoke_task_ids:
            delete_status, _ = api_request(base, f"/api/tasks/{task_id}?delete_files=true", token=token, method="DELETE")
            if delete_status != 200:
                raise RuntimeError(f"task cleanup failed: {task_id}")
        _, remaining = api_request(base, "/api/tasks", token=token)
        if isinstance(remaining, list) and any(item.get("id") in set(smoke_task_ids) for item in remaining):
            raise RuntimeError("deleted smoke tasks remain in the task list")

        completed = True
        return {
            "runtime": runtime_kind,
            "packaged_version": health.get("version", ""),
            "http_pause_resume_bytes": http_output_size,
            "http_sha256_verified": True,
            "http_range_worker_requests": range_stats["requests"],
            "http_distinct_range_spans": range_stats["distinct_spans"],
            "http_max_simultaneous_ranges": range_stats["max_active"],
            "http_no_range_bytes": no_range_output_size,
            "http_no_range_sha256_verified": True,
            "http_signed_refresh_bytes": signed_output_size,
            "http_signed_refresh_sha256_verified": True,
            "hls_output_bytes": hls_output_size,
            "llhls_part_only_output_bytes": llhls_output_size,
            "llhls_part_only_duration_seconds": round(llhls_duration, 3),
            "dash_output_bytes": dash_output_size,
            "first_local_segment_ready_seconds": round(first_local_ready_seconds, 2),
            "local_playback_open_ms": round(playback_open_latency * 1000),
            "local_playlist_load_ms": round(playlist_latency * 1000),
            "initial_playlist_segments": sum(1 for line in playlist_text.splitlines() if line.startswith("segments/")),
            "tasks_deleted": True,
        }
    finally:
        if core is not None and core.poll() is None:
            try:
                config_path = portable / "config.json"
                current = json.loads(config_path.read_text(encoding="utf-8"))
                api_request(
                    f"http://127.0.0.1:{current['port']}", "/api/desktop/core/shutdown",
                    token=str(current["token"]), method="POST", timeout=2,
                )
                core.wait(timeout=8)
            except Exception:
                core.kill()
                core.wait(timeout=5)
        if core_log_stream is not None:
            core_log_stream.flush()
            core_log_stream.close()
        core_log = smoke_root / "core.log"
        if not completed and core_log.is_file():
            # The fixture URLs and token are generated inside this isolated
            # smoke, so its bounded tail is safe and makes packaged-only CI
            # failures diagnosable before the workspace is cleaned.
            tail = core_log.read_bytes()[-32 * 1024 :].decode("utf-8", errors="replace")
            if tail.strip():
                print("\n--- packaged Core smoke log tail ---", file=sys.stderr)
                print(tail.rstrip(), file=sys.stderr)
        if server is not None:
            server.shutdown()
            server.server_close()
        if server_thread is not None:
            server_thread.join(timeout=3)
        shutil.rmtree(smoke_root, ignore_errors=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--archive", type=Path)
    source.add_argument("--source", action="store_true")
    args = parser.parse_args()
    archive = args.archive.resolve() if args.archive else None
    if archive is not None and not archive.is_file():
        raise SystemExit(f"archive not found: {archive}")
    result = run(archive)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
