from __future__ import annotations

import asyncio
import json
import shutil
import socket
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from backend.app.config import settings
from backend.app.downloader.http_file import HTTPDownloader
from backend.app.models import Task, TaskStatus, TaskType
from backend.app.native_engine import (
    begin_native_job_progress,
    locate_native_engine_executable,
    native_job_exit_code,
    start_native_job,
    write_native_job,
)
from backend.app.native_shell import NativeShellSupervisor


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


def test_native_job_exit_code_reads_terminal_progress(tmp_path):
    path = tmp_path / "native-engine.progress.json"
    assert native_job_exit_code(path) is None
    path.write_text('{"status":"downloading","downloaded":10}', encoding="utf-8")
    assert native_job_exit_code(path) is None
    path.write_text('{"status":"done","downloaded":10}', encoding="utf-8")
    assert native_job_exit_code(path) == 0
    path.write_text('{"status":"paused","code":20}', encoding="utf-8")
    assert native_job_exit_code(path) == 20
    path.write_text('{"status":"canceled"}', encoding="utf-8")
    assert native_job_exit_code(path) == 21
    path.write_text('{"status":"error","code":30}', encoding="utf-8")
    assert native_job_exit_code(path) == 30


def test_begin_native_job_progress_clears_terminal_status(tmp_path):
    path = tmp_path / "native-engine.progress.json"
    path.write_text('{"status":"paused","code":20,"downloaded":40}', encoding="utf-8")
    begin_native_job_progress(path)
    assert native_job_exit_code(path) is None
    payload = json.loads(path.read_text(encoding="utf-8"))
    assert payload["status"] == "starting"


def test_start_native_job_clears_stale_paused_progress(tmp_path, monkeypatch):
    exe = tmp_path / "HLSNativeShell.exe"
    exe.write_bytes(b"not-a-real-binary")
    job = tmp_path / "native-engine.job.json"
    progress = tmp_path / "native-engine.progress.json"
    job.write_text("{}", encoding="utf-8")
    progress.write_text('{"status":"paused","code":20,"downloaded":40}', encoding="utf-8")
    spawned = {"called": False}

    def fake_spawn(**_kwargs):
        spawned["called"] = True
        raise AssertionError("resident queue must not spawn --job")

    monkeypatch.setattr("backend.app.native_engine.run_native_engine", fake_spawn)
    shell = NativeShellSupervisor()
    monkeypatch.setattr("backend.app.native_engine.native_shell_supervisor", lambda: shell)
    shell.boot_resident()
    worker = threading.Thread(target=lambda: shell.wait_event(0, 2), daemon=True)
    worker.start()
    deadline = time.monotonic() + 1.0
    while time.monotonic() < deadline and not shell.has_event_poller():
        time.sleep(0.01)
    handle = start_native_job(executable=exe, job_path=job, progress_path=progress)
    worker.join(2.0)
    assert handle is None
    assert spawned["called"] is False
    assert native_job_exit_code(progress) is None
    assert json.loads(progress.read_text(encoding="utf-8"))["status"] == "starting"


def test_await_native_engine_ignores_starting_until_new_terminal(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    task = Task(id="native-await-stale", url="http://127.0.0.1/a.bin", task_type=TaskType.HTTP)
    downloader = HTTPDownloader(task)
    control = tmp_path / "native-engine.control"
    progress = tmp_path / "native-engine.progress.json"
    control.write_text("run", encoding="utf-8")
    progress.write_text('{"status":"paused","code":20}', encoding="utf-8")
    begin_native_job_progress(progress)

    def finish():
        time.sleep(0.35)
        progress.write_text(
            '{"status":"paused","code":20,"downloaded":4}',
            encoding="utf-8",
        )

    threading.Thread(target=finish, daemon=True).start()
    started = time.monotonic()
    assert asyncio.run(downloader._await_native_engine(None, control, progress)) == 20
    assert time.monotonic() - started >= 0.3


def test_start_native_job_queues_when_poller_is_waiting(tmp_path, monkeypatch):
    import time

    exe = tmp_path / "HLSNativeShell.exe"
    exe.write_bytes(b"not-a-real-binary")
    job = tmp_path / "native-engine.job.json"
    progress = tmp_path / "native-engine.progress.json"
    job.write_text("{}", encoding="utf-8")
    spawned = {"called": False}

    def fake_spawn(**_kwargs):
        spawned["called"] = True
        raise AssertionError("resident queue must not spawn --job")

    monkeypatch.setattr("backend.app.native_engine.run_native_engine", fake_spawn)
    shell = NativeShellSupervisor()
    monkeypatch.setattr("backend.app.native_engine.native_shell_supervisor", lambda: shell)
    shell.boot_resident()
    worker = threading.Thread(target=lambda: shell.wait_event(0, 2), daemon=True)
    worker.start()
    deadline = time.monotonic() + 1.0
    while time.monotonic() < deadline and not shell.has_event_poller():
        time.sleep(0.01)
    handle = start_native_job(executable=exe, job_path=job, progress_path=progress)
    worker.join(2.0)
    assert handle is None
    assert spawned["called"] is False
    events = shell.wait_event(0, 0)["events"]
    assert any(item.get("kind") == "http_job" for item in events)


def test_start_native_job_spawns_without_resident_poller(tmp_path, monkeypatch):
    exe = tmp_path / "HLSNativeShell.exe"
    exe.write_bytes(b"not-a-real-binary")
    job = tmp_path / "native-engine.job.json"
    job.write_text("{}", encoding="utf-8")
    called = {}

    def fake_spawn(*, executable, job_path, cwd=None):
        called["executable"] = executable
        called["job_path"] = job_path
        return "spawned"

    monkeypatch.setattr("backend.app.native_engine.run_native_engine", fake_spawn)
    handle = start_native_job(executable=exe, job_path=job, progress_path=tmp_path / "p.json")
    assert handle == "spawned"
    assert called["executable"] == exe
    assert called["job_path"] == job


def test_await_native_engine_reads_progress_without_process(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    task = Task(id="native-await", url="http://127.0.0.1/a.bin", task_type=TaskType.HTTP)
    downloader = HTTPDownloader(task)
    control = tmp_path / "native-engine.control"
    progress = tmp_path / "native-engine.progress.json"
    control.write_text("run", encoding="utf-8")
    progress.write_text(
        '{"status":"done","downloaded":12,"total":12,"speed":0,"code":0}',
        encoding="utf-8",
    )
    assert asyncio.run(downloader._await_native_engine(None, control, progress)) == 0


def test_http_run_does_not_use_native_engine_under_pytest(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    monkeypatch.setattr(settings, "native_http_engine", True)
    task = Task(id="native-off", url="https://files.test/a.bin", task_type=TaskType.HTTP)
    downloader = HTTPDownloader(task)
    assert downloader._native_http_engine_eligible(tmp_path / "http-resume.json") is False


def test_reset_range_state_unlinks_native_ranges_sidecar(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    task = Task(id="native-sidecar", url="http://127.0.0.1/a.bin", task_type=TaskType.HTTP)
    downloader = HTTPDownloader(task)
    part_path = tmp_path / "payload.downloading"
    state_path = tmp_path / "http-resume.json"
    sidecar = tmp_path / "native-engine.ranges.json"
    part_path.write_bytes(b"partial")
    state_path.write_text("{}", encoding="utf-8")
    sidecar.write_text('{"ranges":[[0,1]]}', encoding="utf-8")
    downloader._reset_range_state_for_sequential(part_path, state_path)
    assert not part_path.exists()
    assert not state_path.exists()
    assert not sidecar.exists()


def test_native_pause_exit_does_not_publish_preallocated_file(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    monkeypatch.setattr(settings, "proxy_mode", "direct")
    monkeypatch.setattr(HTTPDownloader, "_native_http_engine_eligible", lambda self, *_args, **_kwargs: True)

    async def fake_native(self, headers, part_path, state_path, metadata):
        total = int(metadata.get("total") or self.task.progress.total_bytes or 0)
        part_path.write_bytes(b"\x00" * total)
        self.task.engine_state["native_exit"] = "paused"
        return True

    monkeypatch.setattr(HTTPDownloader, "_download_with_native_engine", fake_native)
    server, url = _range_server(BODY)
    try:
        task = Task(
            id="native-pause-publish",
            url=url,
            task_type=TaskType.HTTP,
            filename="payload.bin",
            concurrency=4,
        )
        asyncio.run(HTTPDownloader(task).run())
    finally:
        server.shutdown()
    assert task.status is TaskStatus.PAUSED
    assert not task.output_path
    downloads = tmp_path / "downloads"
    if downloads.exists():
        for path in downloads.rglob("*"):
            if path.is_file():
                assert path.stat().st_size == 0


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
    monkeypatch.setenv("HLS_NATIVE_HTTP_SPAWN", "1")
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


def _native_shell_job_cmdlines() -> list[str]:
    proc = Path("/proc")
    if not proc.is_dir():
        return []
    found = []
    for entry in proc.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            raw = (entry / "cmdline").read_bytes()
        except OSError:
            continue
        cmdline = raw.replace(b"\x00", b" ").decode("utf-8", "replace")
        if "--job" in cmdline and "hls-native-shell" in cmdline:
            found.append(cmdline.strip())
    return found


@pytest.mark.skipif(shutil.which("cargo") is None, reason="cargo is required to build the native HTTP engine")
def test_resident_supervisor_downloads_http_without_job_fork(tmp_path, monkeypatch):
    """Prove GET runs in the already-running supervisor, not a second --job process."""
    import subprocess

    from backend.app.native_engine import build_native_engine_debug
    from backend.app.native_shell import native_shell_supervisor
    from tests.test_native_shell_supervisor import (
        _free_port,
        _serve,
        _supervisor_binary,
        _wait_status,
    )

    executable = build_native_engine_debug()
    if executable is None:
        pytest.skip("native HTTP engine failed to build")

    monkeypatch.setenv("HLS_TEST_NATIVE_HTTP", "1")
    monkeypatch.delenv("HLS_NATIVE_HTTP_SPAWN", raising=False)
    monkeypatch.setenv("HLS_NATIVE_ENGINE", str(executable))
    monkeypatch.setattr(settings, "native_http_engine", True)
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    monkeypatch.setattr(settings, "proxy_mode", "direct")

    launches: list[object] = []
    real_start = start_native_job

    def tracking_start(**kwargs):
        handle = real_start(**kwargs)
        launches.append(handle)
        return handle

    def forbid_spawn(**kwargs):
        raise AssertionError(f"resident path spawned --job: {kwargs}")

    monkeypatch.setattr("backend.app.downloader.http_file.start_native_job", tracking_start)
    monkeypatch.setattr("backend.app.native_engine.run_native_engine", forbid_spawn)

    port = _free_port()
    core, core_worker = _serve(port)
    status_path = tmp_path / "shell.json"
    proc = subprocess.Popen(
        [
            str(_supervisor_binary()),
            "--headless",
            "--core-url",
            f"http://127.0.0.1:{port}/api",
            "--token",
            settings.token,
            "--status-path",
            str(status_path),
        ],
        cwd=str(Path(__file__).resolve().parent.parent),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    range_server = None
    try:
        _wait_status(
            status_path,
            lambda body: body.get("resident") and body.get("core_running"),
        )
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline and not native_shell_supervisor().has_event_poller():
            time.sleep(0.02)
        poller = native_shell_supervisor().has_event_poller()
        waiter_status = native_shell_supervisor().status()
        assert poller is True, f"supervisor is not polling events: {waiter_status}"

        before_jobs = _native_shell_job_cmdlines()
        range_server, url = _range_server(BODY)
        task = Task(
            id="native-resident",
            url=url,
            task_type=TaskType.HTTP,
            filename="payload.bin",
            concurrency=4,
        )
        asyncio.run(HTTPDownloader(task).run())
        after_jobs = _native_shell_job_cmdlines()
        new_jobs = [item for item in after_jobs if item not in before_jobs]

        assert launches, "HTTPDownloader never called start_native_job"
        assert launches == [None] * len(launches), f"expected in-process queue, got {launches!r}"
        assert task.status is TaskStatus.DONE, task.error_message
        assert task.engine_state.get("http_engine") == "native-shell"
        assert Path(task.output_path).read_bytes() == BODY
        assert new_jobs == [], f"saw --job processes: {new_jobs}"
        assert proc.poll() is None, "resident supervisor exited during the download"
    finally:
        if range_server is not None:
            range_server.shutdown()
        if proc.poll() is None:
            try:
                if proc.stdin is not None:
                    proc.stdin.write('{"op":"shutdown"}\n')
                    proc.stdin.flush()
            except OSError:
                pass
            try:
                proc.wait(timeout=3)
            except Exception:
                proc.kill()
        core.should_exit = True
        core_worker.join(timeout=8)
