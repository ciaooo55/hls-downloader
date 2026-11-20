"""Headless Core/preflight integration; never touches a user's installation."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import traceback

from v7_core_ipc import connect, free_port, send_frame, snapshot


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", required=True, type=Path)
    parser.add_argument("--helper", required=True, type=Path)
    parser.add_argument("--root", required=True, type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    root = args.root.resolve()
    assert root.is_relative_to(repo / ".tool-cache" / "test-tmp"), root
    root.mkdir(parents=True, exist_ok=False)
    processes = []
    result = {"passed": False, "steps": []}

    def start(label: str, installation: str | None = None):
        install = root / (installation or label)
        resources = install / "app" / "resources"
        resources.mkdir(parents=True, exist_ok=True)
        executable = resources / "HLSDownloaderEngine.exe"
        if not executable.exists():
            shutil.copyfile(args.engine, executable)
            (install / "HLSDownloader.exe").write_bytes(b"test installation marker")
        port = free_port()
        env = os.environ.copy()
        env.update({
            "HLS_V7_PIPE": rf"\\.\pipe\HLSShutdownFixture-{os.getpid()}-{label}",
            "HLS_V7_CORE_TCP": "1", "HLS_V7_CORE_BIND": f"127.0.0.1:{port}",
            "HLS_V7_DATA_DIR": str(root / label / "data"),
            "HLS_V7_DOWNLOAD_DIR": str(root / label / "downloads"),
            "TEMP": str(root), "TMP": str(root),
        })
        with (root / f"{label}-{len(processes)}.log").open("wb") as log:
            child = subprocess.Popen([str(executable), "--no-tray"], env=env, cwd=install,
                                     stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                     creationflags=subprocess.CREATE_NO_WINDOW)
        processes.append((child, port))
        with connect(port):
            pass
        # The maintenance pipe is published after normal IPC binds.
        deadline = time.monotonic() + 5
        while True:
            try:
                with open(rf"\\.\pipe\HLSDownloader.v7.maintenance.{child.pid}", "r+b", buffering=0):
                    break
            except OSError:
                assert child.poll() is None, child.returncode
                if time.monotonic() >= deadline:
                    raise TimeoutError("Maintenance pipe was not published")
                time.sleep(0.02)
        return child, port, install, env

    try:
        target, target_port, install, target_env = start("target")
        second, _, _, _ = start("target-second", "target")
        other, other_port, _, other_env = start("other")
        with connect(target_port) as ipc:
            created = send_frame(ipc, {
                "type": "command", "request_id": 1, "command": {"kind": "create_task", "spec": {
                    "url": "http://127.0.0.1:9/preserved.bin", "filename": "preserved.bin",
                    "resource_kind": "file", "start_paused": True, "expected_size": 4,
                }}
            })
            assert created["type"] == "events", created
            task = snapshot(ipc, "task-1", 2)
            assert task["status"] == "paused", task
        helper_env = target_env.copy()
        helper_env["HLS_V7_PIPE"] = other_env["HLS_V7_PIPE"]
        with (root / "preflight.log").open("wb") as log:
            helper = subprocess.run([str(args.helper.resolve()), str(install)], env=helper_env,
                                    stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                    timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
        assert helper.returncode == 0, helper.returncode
        target.wait(timeout=5)
        second.wait(timeout=5)
        assert target.returncode == 0, target.returncode
        assert second.returncode == 0, second.returncode
        assert other.poll() is None, "Preflight stopped a sibling installation"
        result["steps"].append("both private Cores in target installation stopped; sibling untouched")
        with connect(other_port) as ipc:
            assert send_frame(ipc, {"type": "snapshot", "request_id": 3})["type"] == "snapshot"
        restarted, restarted_port, _, _ = start("target")
        with connect(restarted_port) as ipc:
            restored = snapshot(ipc, "task-1", 4)
            assert restored["status"] == "paused", restored
            assert restored["filename"] == task["filename"], restored
        result["steps"].append("Core reopened its persisted paused task after preflight")
        result["passed"] = True
    except Exception as error:
        result.update(error=repr(error), traceback=traceback.format_exc())
    finally:
        for child, port in processes:
            if child.poll() is None:
                try:
                    with connect(port) as ipc:
                        send_frame(ipc, {"type": "command", "request_id": 100,
                                         "command": {"kind": "shutdown"}})
                    child.wait(timeout=15)
                except Exception as error:
                    result["cleanup_error"] = repr(error)
                    child.kill()
                    child.wait(timeout=5)
        (root / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
