import json
import os
import socket
import subprocess
import threading
import time
import urllib.request
from pathlib import Path

from backend.app.config import settings
from backend.app.main import app


ROOT = Path(__file__).resolve().parent.parent
AUTH_TOKEN = settings.token
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def _supervisor_binary() -> Path:
    name = "hls-native-shell.exe" if os.name == "nt" else "hls-native-shell"
    path = ROOT / "native_shell" / "target" / "debug" / name
    if not path.is_file():
        built = subprocess.run(
            ["cargo", "build", "--offline"],
            cwd=ROOT / "native_shell",
            check=False,
            capture_output=True,
            text=True,
        )
        if built.returncode != 0:
            built = subprocess.run(
                ["cargo", "build"],
                cwd=ROOT / "native_shell",
                check=True,
                capture_output=True,
                text=True,
            )
    assert path.is_file(), f"missing supervisor binary at {path}"
    return path


def _free_port() -> int:
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
    listener.close()
    return port


def _serve(port: int):
    import asyncio
    import uvicorn

    server = uvicorn.Server(
        uvicorn.Config(app, host="127.0.0.1", port=port, log_level="error", lifespan="on")
    )

    def run() -> None:
        asyncio.run(server.serve())

    worker = threading.Thread(target=run, name="native-shell-core", daemon=True)
    worker.start()
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        try:
            OPENER.open(f"http://127.0.0.1:{port}/api/health", timeout=0.4).read()
            return server, worker
        except Exception:
            time.sleep(0.05)
    server.should_exit = True
    raise AssertionError("download core did not start")


def _request(port: int, method: str, path: str, payload: dict | None = None) -> dict:
    body = json.dumps(payload).encode("utf-8") if payload is not None else None
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/api{path}",
        data=body,
        method=method,
        headers={"X-Token": AUTH_TOKEN, "Content-Type": "application/json"},
    )
    with OPENER.open(request, timeout=8) as response:
        return json.loads(response.read().decode("utf-8"))


def _wait_status(path: Path, predicate, timeout: float = 8.0) -> dict:
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        if path.exists():
            try:
                last = json.loads(path.read_text(encoding="utf-8"))
                if predicate(last):
                    return last
            except json.JSONDecodeError:
                pass
        time.sleep(0.02)
    raise AssertionError(f"supervisor status not reached: {last}")


def test_supervisor_binary_shows_precreated_confirm_from_core_snapshot(tmp_path: Path):
    port = _free_port()
    server, worker = _serve(port)
    status_path = tmp_path / "shell.json"
    proc = subprocess.Popen(
        [
            str(_supervisor_binary()),
            "--headless",
            "--core-url",
            f"http://127.0.0.1:{port}/api",
            "--token",
            AUTH_TOKEN,
            "--status-path",
            str(status_path),
        ],
        cwd=str(ROOT),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        booted = _wait_status(
            status_path,
            lambda body: body.get("resident")
            and body.get("core_running")
            and body.get("tray")
            and body.get("windows", {}).get("handoff", {}).get("created")
            and not body.get("windows", {}).get("handoff", {}).get("visible"),
        )
        assert booted["created_at_boot"] is True
        assert booted["windows"]["progress"]["created"] is True
        assert booted["windows"]["complete"]["created"] is True
        assert booted["windows"]["progress"]["focusable"] is False

        presenter = _request(port, "GET", "/browser/presenter")
        assert presenter["mode"] == "native-shell"
        assert presenter["ready"] is True

        started = time.monotonic()
        handoff = _request(
            port,
            "POST",
            "/browser/handoffs",
            {
                "url": "https://cdn.example.test/setup.exe",
                "filename": "setup.exe",
                "size": 4096,
                "cookie": "must-not-leave-the-core",
            },
        )
        shown = _wait_status(
            status_path,
            lambda body: body.get("windows", {}).get("handoff", {}).get("visible")
            and (body.get("snapshot") or {}).get("filename") == "setup.exe",
        )
        elapsed_ms = (time.monotonic() - started) * 1000
        assert handoff["presentation_mode"] == "native-shell"
        assert shown["snapshot"]["url"] == "https://cdn.example.test/setup.exe"
        assert shown["snapshot"]["size"] == 4096
        assert "cookie" not in shown["snapshot"]
        assert shown["windows"]["handoff"]["created"] is True
        assert elapsed_ms < 1500

        proc.stdin.write('{"op":"hide_main"}\n')
        proc.stdin.flush()
        hidden = _wait_status(
            status_path,
            lambda body: body.get("resident") and not body.get("main_open"),
        )
        assert hidden["tray"] is True
        assert hidden["windows"]["handoff"]["created"] is True

        rejected = _request(port, "POST", f"/browser/handoffs/{handoff['id']}/reject")
        assert rejected["status"] == "rejected"
        proc.stdin.write('{"op":"reject"}\n')
        proc.stdin.flush()
        closed = _wait_status(
            status_path,
            lambda body: not body.get("windows", {}).get("handoff", {}).get("visible"),
        )
        assert closed["resident"] is True
    finally:
        if proc.poll() is None:
            try:
                proc.stdin.write('{"op":"shutdown"}\n')
                proc.stdin.flush()
            except OSError:
                pass
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
        server.should_exit = True
        worker.join(timeout=8)
