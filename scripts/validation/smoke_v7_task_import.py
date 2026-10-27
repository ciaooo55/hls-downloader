"""Exercise task-list round trips and partial imports through the actual Core IPC."""
from __future__ import annotations

import argparse
import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import traceback
import uuid

from msi_checkpoint_fixture import connect, send, wait_task


def run(args):
    if os.environ.get("HLS_VM_ACCEPTANCE") != "1":
        raise RuntimeError("Run only in Windows Sandbox")
    result = {"passed": False, "checks": [], "engine_sha256": hashlib.sha256(args.engine.read_bytes()).hexdigest()}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    payload = bytes(range(256)) * 4096

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("Content-Type", "application/octet-stream")
            self.end_headers()
            self.wfile.write(payload)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    def record(name, **details):
        result["checks"].append({"name": name, **details})
        print(json.dumps(result["checks"][-1]), flush=True)

    with tempfile.TemporaryDirectory(prefix="hls-task-import-") as temporary:
        root = Path(temporary)

        @contextlib.contextmanager
        def engine(name):
            with socket.socket() as probe:
                probe.bind(("127.0.0.1", 0))
                port = probe.getsockname()[1]
            env = {**os.environ, "HLS_V7_DATA_DIR": str(root / name / "data"),
                   "HLS_V7_DOWNLOAD_DIR": str(root / name / "downloads"),
                   "HLS_V7_PIPE": rf"\\.\pipe\HLSDownloader.import-{uuid.uuid4().hex}",
                   "HLS_V7_CORE_TCP": "1", "HLS_V7_CORE_BIND": f"127.0.0.1:{port}"}
            with args.report.with_suffix(f".{name}.log").open("wb") as log:
                process = subprocess.Popen([str(args.engine.resolve())], env=env, stdout=log, stderr=log,
                                           creationflags=subprocess.CREATE_NO_WINDOW)
                try:
                    with connect(port) as ipc:
                        ipc.settimeout(180)
                        response = send(ipc, {"type": "store_settings", "request_id": 1, "values": {
                            "legal_terms_accepted": True, "download_complete_popup_enabled": False,
                            "download_progress_window_enabled": False, "av_scan_enabled": False}})
                        assert response["type"] != "error", response
                        yield ipc
                finally:
                    process.terminate()
                    process.wait(timeout=15)

        def command(ipc, kind, **fields):
            return send(ipc, {"type": "command", "request_id": 2, "command": {"kind": kind, **fields}})

        def created(response):
            assert response["type"] == "events", response
            return [event["event"]["snapshot"] for event in response["events"]
                    if event["event"]["kind"] == "task_created"]

        try:
            tasks = [{"url": f"https://cdn.test/library-{index}.bin", "filename": f"library-{index}.bin",
                      "scheduled_start_at": "2999-01-01T00:00:00Z"} for index in range(1000)]
            source = root / "library.json"
            source.write_text(json.dumps({"schema": "hls-downloader.tasks.v1", "tasks": tasks}), encoding="utf8")
            with engine("source") as ipc:
                imported = created(command(ipc, "import_paths", paths=[str(source)]))
                assert len(imported) == 1000, len(imported)
                response = command(ipc, "export_tasks", task_ids=[], format="json")
                assert response["type"] == "events", response
                data = next(event["event"]["data"] for event in response["events"]
                            if event["event"]["kind"] == "task_export")
                assert len(json.loads(data)["tasks"]) == 1000
                exported = root / "exported.json"
                exported.write_text(data, encoding="utf8")
                record("actual-ipc-thousand-task-import-and-export", exported_bytes=exported.stat().st_size)
            with engine("target") as ipc:
                response = command(ipc, "import_paths", paths=[str(exported)])
                imported = created(response)
                record("reimport-diagnostics", created_count=len(imported),
                       errors=[event["event"] for event in response.get("events", []) if event["event"]["kind"] == "error"],
                       stored_count=len(send(ipc, {"type":"snapshot", "request_id":3}).get("tasks", [])))
                assert len(imported) == 1000, len(imported)
                assert {task["url"] for task in imported} == {task["url"] for task in tasks}
                assert all(task["scheduled_start_at"] == "2999-01-01T00:00:00Z" for task in imported)
                record("own-export-reimports-into-a-fresh-task-store-without-loss")
                paths = []
                for index in range(3):
                    path = root / f"batch-{index}.json"
                    path.write_text("invalid JSON" if index == 1 else json.dumps({
                        "schema": "hls-downloader.tasks.v1", "tasks": [{
                            "url": f"http://127.0.0.1:{server.server_port}/actual-{index}.bin"}]}), encoding="utf8")
                    paths.append(str(path))
                response = command(ipc, "import_paths", paths=paths)
                imported = created(response)
                assert len(imported) == 2, response
                errors = [event["event"] for event in response["events"] if event["event"]["kind"] == "error"]
                assert len(errors) == 1 and errors[0]["code"] == "import_partial_failure", errors
                assert "2 项任务" in errors[0]["message"] and "第 2 项" in errors[0]["message"], errors
                for task in imported:
                    done = wait_task(ipc, task["task_id"], lambda item: item["status"] in ("completed", "failed"), 10)
                    assert done["status"] == "completed", done
                    assert hashlib.sha256(Path(done["output_path"]).read_bytes()).digest() == hashlib.sha256(payload).digest()
                record("bad-middle-file-reported-both-good-files-downloaded-with-correct-sha256", message=errors[0]["message"])
                portable = root / "foreign-queue.json"
                portable.write_text(json.dumps({"schema":"hls-downloader.tasks.v1", "tasks":[{
                    "url":f"http://127.0.0.1:{server.server_port}/foreign-queue.bin",
                    "queue_id":"not-on-this-computer", "scheduled_start_at":"2999-01-01T00:00:00Z"
                }]}), encoding="utf8")
                imported_queue = created(command(ipc, "import_paths", paths=[str(portable)]))
                assert len(imported_queue) == 1 and imported_queue[0]["queue_id"] == "default", imported_queue
                record("foreign-queue-falls-back-to-default-without-losing-task")
                post = root / "missing-post-body.json"
                post.write_text(json.dumps({"schema":"hls-downloader.tasks.v1", "tasks":[{
                    "url":f"http://127.0.0.1:{server.server_port}/must-not-request",
                    "request_method":"POST"
                }]}), encoding="utf8")
                response = command(ipc, "import_paths", paths=[str(post)])
                assert created(response) == [], response
                errors = [event["event"] for event in response["events"] if event["event"]["kind"] == "error"]
                assert len(errors) == 1 and "POST" in errors[0]["message"], response
                record("missing-post-body-rejected-before-task-creation", message=errors[0]["message"])
            result["passed"] = True
        except Exception as error:
            result["error"] = str(error)
            result["traceback"] = traceback.format_exc()
        finally:
            server.shutdown()
            server.server_close()
            args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf8")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    raise SystemExit(run(parser.parse_args()))
