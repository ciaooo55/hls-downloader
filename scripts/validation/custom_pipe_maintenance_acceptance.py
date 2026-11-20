"""Real candidate MSI repair with a private Core endpoint unknown to msiexec."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import time
import traceback

from v7_core_ipc import connect, free_port, send_frame, snapshot


def main() -> int:
    assert os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_OS") == "Windows"
    parser = argparse.ArgumentParser()
    parser.add_argument("--msi", type=Path, required=True)
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--report-root", type=Path, required=True)
    args = parser.parse_args()
    root = args.report_root.resolve()
    root.mkdir(parents=True, exist_ok=True)
    data_root = root / "checkpoint"
    state = data_root / "state.json"
    core_port, origin_port = free_port(), free_port()
    env = os.environ.copy()
    env.update({
        "HLS_V7_PIPE": rf"\\.\pipe\HLSRepairPrivate-{os.getpid()}",
        "HLS_V7_DATA_DIR": str(data_root / "data"),
        "HLS_V7_DOWNLOAD_DIR": str(data_root / "downloads"),
        "HLS_V7_CORE_TCP": "1",
        "HLS_V7_CORE_BIND": f"127.0.0.1:{core_port}",
    })
    result = {"passed": False, "candidate_msi": str(args.msi), "steps": []}
    engine = None

    def checkpoint(mode: str) -> dict:
        report = root / f"{mode}.json"
        command = [
            os.sys.executable, str(Path(__file__).with_name("msi_checkpoint_fixture.py")),
            "--mode", mode, "--engine", str(args.engine), "--root", str(data_root),
            "--state", str(state), "--report", str(report),
            "--core-port", str(core_port), "--origin-port", str(origin_port),
        ]
        with (root / f"{mode}.log").open("wb") as log:
            subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=240)
        return json.loads(report.read_text(encoding="utf-8"))

    try:
        before = checkpoint("create-pause")
        engine_log = (root / "resident-core.log").open("wb")
        with engine_log:
            engine = subprocess.Popen(
                [str(args.engine), "--no-tray"], env=env,
                stdin=subprocess.DEVNULL, stdout=engine_log, stderr=subprocess.STDOUT,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
            with connect(core_port) as ipc:
                task = snapshot(ipc, before["task_id"], 1)
                assert task["status"] == "paused", task
            # Exercise the real maintenance endpoint's narrow command contract.
            maintenance = rf"\\.\pipe\HLSDownloader.v7.maintenance.{engine.pid}"
            deadline = time.monotonic() + 5
            while True:
                try:
                    pipe = open(maintenance, "r+b", buffering=0)
                    break
                except OSError:
                    assert engine.poll() is None, "Candidate Core exited before maintenance readiness"
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.05)
            with pipe:
                request = json.dumps({"type": "snapshot", "request_id": 99}).encode()
                pipe.write(struct.pack("<I", len(request)) + request)
                header = pipe.read(4)
                assert len(header) == 4, header
                size = struct.unpack("<I", header)[0]
                assert 0 < size < 4096, size
                response = json.loads(pipe.read(size))
                assert response["type"] == "error" and response["code"] == "unsupported_command", response
            assert engine.poll() is None, "Non-shutdown request stopped Core"
            result["steps"].append("maintenance rejects ordinary IPC")
            # The installer receives neither the private pipe nor the TCP
            # endpoint. It must find and verify the Core through its PID.
            installer_env = {key: value for key, value in os.environ.items()
                             if key not in {"HLS_V7_PIPE", "HLS_V7_CORE_TCP", "HLS_V7_CORE_BIND"}}
            repair = subprocess.run(
                ["msiexec.exe", "/i", str(args.msi.resolve()), "/qn", "/norestart",
                 "REINSTALL=ALL", "REINSTALLMODE=vomus", "/l*v", str(root / "repair.log")],
                env=installer_env, stdin=subprocess.DEVNULL, timeout=90,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )
            assert repair.returncode in {0, 3010}, repair.returncode
            engine.wait(timeout=5)
            assert engine.returncode == 0, engine.returncode
            result["steps"].append("candidate MSI repair stops private Core gracefully")
        after = checkpoint("verify-resume")
        assert after["task_id"] == before["task_id"] and after["status"] == "completed", after
        assert after["file_sha256"] == after["expected_sha256"], after
        result.update(passed=True, before=before, after=after)
    except Exception as error:
        result.update(error=repr(error), traceback=traceback.format_exc())
    finally:
        if engine is not None and engine.poll() is None:
            try:
                with connect(core_port) as ipc:
                    send_frame(ipc, {"type": "command", "request_id": 100,
                                     "command": {"kind": "shutdown"}})
                engine.wait(timeout=15)
            except Exception as error:
                # This process belongs solely to this disposable CI fixture.
                result["cleanup_error"] = repr(error)
                engine.kill()
                engine.wait(timeout=5)
        (root / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
