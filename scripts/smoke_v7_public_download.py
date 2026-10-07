"""Download a public media file through the real Core and verify its bytes."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from urllib.request import urlopen

from msi_checkpoint_fixture import connect, send, wait_task
from smoke_v7_transfer_performance import free_port


def run(args):
    url = "https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.webm"
    with urlopen(url, timeout=30) as response:
        expected = response.read(8 * 1024 * 1024 + 1)
    if not expected or len(expected) > 8 * 1024 * 1024:
        raise ValueError("Unexpected public media fixture size")
    expected_hash = hashlib.sha256(expected).hexdigest()
    temp_root = Path(__file__).resolve().parents[1] / ".tool-cache" / "test-tmp"
    temp_root.mkdir(parents=True, exist_ok=True)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    result = {"passed": False, "url": url, "expected_bytes": len(expected), "expected_sha256": expected_hash,
              "engine_sha256": hashlib.sha256(args.engine.read_bytes()).hexdigest()}
    with tempfile.TemporaryDirectory(prefix="public-download-", dir=temp_root) as temporary:
        root = Path(temporary)
        engine_path = root / "HLSDownloaderEngine.exe"
        shutil.copy2(args.engine, engine_path)
        port = free_port()
        env = os.environ.copy()
        env.update(HLS_V7_DATA_DIR=str(root / "data"), HLS_V7_DOWNLOAD_DIR=str(root / "downloads"),
                   HLS_V7_CORE_TCP="1", HLS_V7_CORE_BIND=f"127.0.0.1:{port}",
                   HLS_V7_PIPE=rf"\\.\pipe\HLSPublicDownload-{os.getpid()}")
        engine = subprocess.Popen([str(engine_path)], env=env, stdout=subprocess.DEVNULL,
                                  stderr=subprocess.DEVNULL, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        try:
            with connect(port) as ipc:
                send(ipc, {"type": "store_setting", "request_id": 1, "key": "download_dir", "value": str(root / "downloads")})
                created = send(ipc, {"type": "command", "request_id": 2, "command": {"kind": "create_task", "spec": {
                    "url": url, "resource_kind": "file", "title": "Public MDN video", "filename": "flower.webm", "expected_size": len(expected), "concurrency": 4,
                }}})
                if "events" not in created:
                    result["create_response"] = created
                    raise RuntimeError("Core rejected the public download task")
                task_id = next(e["event"]["snapshot"]["task_id"] for e in created["events"] if e["event"].get("kind") == "task_created")
                send(ipc, {"type": "command", "request_id": 3, "command": {"kind": "task_action", "task_id": task_id, "action": "start"}})
                task = wait_task(ipc, task_id, lambda t: t["status"] in {"completed", "failed"}, 4)
                result["status"] = task["status"]
                if task["status"] != "completed":
                    result["failure"] = task.get("failure")
                    result["log_tail"] = task.get("log_tail")
                    raise RuntimeError("Core did not complete the public media download")
                output = Path(task["output_path"])
                result["actual_bytes"] = output.stat().st_size
                result["actual_sha256"] = hashlib.sha256(output.read_bytes()).hexdigest()
                result["passed"] = result["actual_bytes"] == len(expected) and result["actual_sha256"] == expected_hash
                if not result["passed"]: raise AssertionError("Public media bytes differ from the source")
        except Exception as error:
            result["error"] = str(error)
        finally:
            engine.terminate()
            try: engine.wait(timeout=10)
            except subprocess.TimeoutExpired: engine.kill(); engine.wait(timeout=5)
            args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    raise SystemExit(0 if run(parser.parse_args())["passed"] else 1)
