"""Sandbox-only regression: stop/resume HTTP downloads while the origin sends nothing."""
from __future__ import annotations

import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import subprocess
import threading
import time
import traceback
import uuid

from msi_checkpoint_fixture import connect, send


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--throttle-only", action="store_true")
    args = parser.parse_args()
    assert os.environ.get("HLS_VM_ACCEPTANCE") == "1", "Windows Sandbox only"
    args.root.mkdir(parents=True, exist_ok=False)
    downloads = args.root / "downloads"
    body = bytes(range(256)) * (8 * 1024 * 1024 // 256)
    prefix_size = 128 * 1024
    media_body = b""
    if args.throttle_only:
        media = args.root / "source.ts"
        subprocess.run([str(args.engine.parent / "ffmpeg.exe"), "-nostdin", "-v", "error", "-y",
                        "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25", "-t", "3",
                        "-c:v", "mpeg2video", "-f", "mpegts", str(media)], check=True, timeout=30,
                       creationflags=subprocess.CREATE_NO_WINDOW)
        media_body = media.read_bytes()
    gates: dict[str, threading.Event] = {}
    requests: list[dict] = []
    report = {"passed": False, "checks": [], "engine_sha256": hashlib.sha256(args.engine.read_bytes()).hexdigest()}

    def wait(predicate, label, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = predicate()
            if result:
                return result
            time.sleep(.025)
        raise AssertionError(label)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_HEAD(self):
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Accept-Ranges", "bytes")
            self.end_headers()

        def do_GET(self):
            payload = (b"#EXTM3U\n#EXT-X-TARGETDURATION:3\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:3,\nsource.ts\n#EXT-X-ENDLIST\n"
                       if self.path.endswith(".m3u8") else media_body if self.path.endswith(".ts") else body)
            start, end = 0, len(payload) - 1
            requested = self.headers.get("Range")
            if requested:
                first, last = requested.removeprefix("bytes=").split("-", 1)
                start, end = int(first), min(int(last) if last else end, end)
            requests.append({"path": self.path, "start": start, "end": end})
            self.send_response(206 if requested else 200)
            self.send_header("Content-Length", str(end - start + 1))
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("ETag", '"stalled-http-fixture"')
            if requested:
                self.send_header("Content-Range", f"bytes {start}-{end}/{len(payload)}")
            self.end_headers()
            try:
                split = min(start + prefix_size, end + 1)
                self.wfile.write(payload[start:split])
                self.wfile.flush()
                if split <= end:
                    gates[self.path].wait(90)
                    self.wfile.write(payload[split:end + 1])
            except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env = {**os.environ, "HLS_V7_DATA_DIR": str(args.root / "data"),
           "HLS_V7_DOWNLOAD_DIR": str(downloads), "HLS_V7_CORE_TCP": "1",
           "HLS_V7_PIPE": rf"\\.\pipe\HLSDownloader.stall-{uuid.uuid4().hex}",
           "HLS_V7_CORE_BIND": f"127.0.0.1:{port}", "HLS_V6_SKIP_MIGRATE": "1"}
    log = args.report.with_suffix(".engine.log").open("wb")
    engine = subprocess.Popen([str(args.engine)], env=env, stdout=log, stderr=log,
                              creationflags=subprocess.CREATE_NO_WINDOW)
    ipc = None
    try:
        ipc = connect(port)
        settings = send(ipc, {"type": "store_settings", "request_id": 1, "values": {
            "download_dir": str(downloads), "http_chunk_size_mb": 1, "keep_temp_files": False,
            "download_progress_window_enabled": False, "download_complete_popup_enabled": False,
            "av_scan_enabled": False, "auto_retry_failed_max": 0}})
        assert settings.get("type") != "error", settings

        def command(value):
            response = send(ipc, {"type": "command", "request_id": 2, "command": value})
            assert response.get("type") != "error", response
            return response

        def tasks():
            return send(ipc, {"type": "snapshot", "request_id": 3})["tasks"]

        def task(tid):
            return next(t for t in tasks() if t["task_id"] == tid)

        def action(tid, value):
            return command({"kind": "task_action", "task_id": tid, "action": value})

        for name, connections in ([] if args.throttle_only else [("single", 1), ("parallel", 4), ("delete", 4), ("gap", 1)]):
            path = "/" + name
            gates[path] = threading.Event()
            created = command({"kind": "create_task", "spec": {
                "url": f"http://127.0.0.1:{server.server_port}{path}", "resource_kind": "file",
                "filename": name + ".bin", "concurrency": connections,
                "proxy": "hls-downloader://direct-proxy"}})
            tid = next(e["event"]["snapshot"]["task_id"] for e in created["events"]
                       if e["event"]["kind"] == "task_created")
            action(tid, "start")
            wait(lambda: task(tid)["downloaded_bytes"] >= prefix_size, name + ": no prefix")
            # 故意跨越多个轮询超时，服务端保持连接但不再发送任何数据。
            time.sleep(.8)
            assert task(tid)["status"] == "downloading", task(tid)
            cache = downloads / ".hls-tasks" / tid
            assert cache.is_dir(), cache
            if name == "delete":
                started = time.monotonic()
                action(tid, "delete")
                elapsed = time.monotonic() - started
                assert elapsed < 3, elapsed
                assert all(t["task_id"] != tid for t in tasks())
                assert not cache.exists(), cache
                assert not (downloads / (name + ".bin")).exists()
                report["checks"].append({"name": "delete while all connections are stalled removes task/cache", "seconds": elapsed})
            else:
                if name != "gap":
                    started = time.monotonic()
                    action(tid, "pause")
                    wait(lambda: task(tid)["status"] == "paused" and task(tid)["active_workers"] == 0,
                         name + ": worker did not stop", 3)
                    stopped = time.monotonic() - started
                    assert cache.is_dir(), "pause must preserve resumable bytes"
                    resume_index = len(requests)
                    started = time.monotonic()
                    action(tid, "resume")
                    wait(lambda: task(tid)["status"] == "downloading" and task(tid)["active_workers"] > 0,
                         name + ": resume is stuck", 3)
                    resumed = time.monotonic() - started
                    wait(lambda: any(r["path"] == path and r["start"] > 0 for r in requests[resume_index:]),
                         name + ": did not reuse checkpoint")
                    report["checks"].append({"name": name + ": stalled pause/resume", "pause_seconds": stopped,
                                             "resume_seconds": resumed})
                gates[path].set()
                wait(lambda: task(tid)["status"] in {"completed", "failed"}, name + ": not finished", 30)
                assert task(tid)["status"] == "completed", task(tid)
                wait(lambda: task(tid)["active_workers"] == 0 and not cache.exists(), name + ": cache retained")
                output = downloads / (name + ".bin")
                assert output.read_bytes() == body, name + ": output differs"
                report["checks"].append({"name": name + ": complete, SHA-256 verified, cache removed",
                                         "sha256": hashlib.sha256(output.read_bytes()).hexdigest()})
            gates[path].set()
        if args.throttle_only:
            for name, connections, quota in [("slow-single", 1, False), ("slow-parallel", 4, False),
                                               ("quota-pause", 1, True), ("quota-disable", 4, True)]:
                path = "/" + name
                gates[path] = threading.Event()
                gates[path].set()
                response = send(ipc, {"type": "store_settings", "request_id": 1, "values": {
                    "download_hourly_quota_mib": 1 if quota else 0}})
                assert response.get("type") != "error", response
                created = command({"kind": "create_task", "spec": {
                    "url": f"http://127.0.0.1:{server.server_port}{path}", "resource_kind": "file",
                    "filename": name + ".bin", "concurrency": connections,
                    "speed_limit_kib": 0 if quota else 1, "proxy": "hls-downloader://direct-proxy"}})
                tid = next(e["event"]["snapshot"]["task_id"] for e in created["events"]
                           if e["event"]["kind"] == "task_created")
                action(tid, "start")
                wait(lambda: any(r["path"] == path for r in requests), name + ": no transfer")
                if quota and name == "quota-pause":
                    wait(lambda: task(tid)["downloaded_bytes"] >= 1024 * 1024, "quota was not reached")
                time.sleep(.35)
                started = time.monotonic()
                if name == "quota-disable":
                    response = send(ipc, {"type": "store_settings", "request_id": 1, "values": {
                        "download_hourly_quota_mib": 0}})
                    assert response.get("type") != "error", response
                else:
                    action(tid, "pause")
                    wait(lambda: task(tid)["status"] == "paused" and task(tid)["active_workers"] == 0,
                         name + ": throttled worker did not stop", 3)
                    assert (downloads / ".hls-tasks" / tid).is_dir(), "pause lost resumable cache"
                elapsed = time.monotonic() - started
                if name != "quota-disable":
                    action(tid, "resume")
                    assert time.monotonic() - started < 3, name + ": resume waited for a throttled worker"
                if name.startswith("slow"):
                    action(tid, "speed:0")
                send(ipc, {"type": "store_settings", "request_id": 1, "values": {"download_hourly_quota_mib": 0}})
                wait(lambda: task(tid)["status"] in {"completed", "failed"}, name + ": not finished", 15)
                assert task(tid)["status"] == "completed", task(tid)
                output = Path(task(tid)["output_path"])
                assert output.read_bytes() == body, name + ": resumed output differs"
                wait(lambda: task(tid)["active_workers"] == 0 and not (downloads / ".hls-tasks" / tid).exists(),
                     name + ": cache retained")
                report["checks"].append({"name": name + ": responsive control, resume hash and cache cleanup",
                                         "control_seconds": elapsed, "sha256": hashlib.sha256(body).hexdigest()})
            for name, kind in [("slow-delete", "file"), ("slow-hls", "hls")]:
                path = "/" + name + ("/index.m3u8" if kind == "hls" else "")
                for key in (path, "/slow-hls/source.ts"):
                    gates[key] = threading.Event()
                    gates[key].set()
                created = command({"kind": "create_task", "spec": {
                    "url": f"http://127.0.0.1:{server.server_port}{path}", "resource_kind": kind,
                    "filename": name + (".mp4" if kind == "hls" else ".bin"), "concurrency": 2,
                    "speed_limit_kib": 1, "proxy": "hls-downloader://direct-proxy"}})
                tid = next(e["event"]["snapshot"]["task_id"] for e in created["events"]
                           if e["event"]["kind"] == "task_created")
                action(tid, "start")
                wait(lambda: any(r["path"] == ("/slow-hls/source.ts" if kind == "hls" else path)
                                 for r in requests), name + ": no body request", 15)
                time.sleep(.35)
                cache = downloads / ".hls-tasks" / tid
                assert cache.is_dir(), cache
                started = time.monotonic()
                action(tid, "delete")
                elapsed = time.monotonic() - started
                assert elapsed < 3, (name, elapsed)
                assert all(t["task_id"] != tid for t in tasks())
                assert not cache.exists(), cache
                assert not (downloads / (name + (".mp4" if kind == "hls" else ".bin"))).exists()
                report["checks"].append({"name": name + ": delete stops throttled worker and removes cache",
                                         "control_seconds": elapsed})
        report["passed"] = True
    except Exception:
        report["error"] = traceback.format_exc()
        report["requests"] = requests.copy()
        if ipc:
            try:
                report["failure_tasks"] = tasks()
            except Exception:
                pass
    finally:
        for gate in gates.values():
            gate.set()
        if ipc:
            ipc.close()
        engine.terminate()
        engine.wait(timeout=10)
        log.close()
        server.shutdown()
        args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
