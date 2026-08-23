from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import json
import math
import os
import shutil
import struct
import subprocess
import tempfile
import threading
import time
import uuid
from pathlib import Path


def frame(message: dict[str, object]) -> bytes:
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    return struct.pack("<I", len(payload)) + payload


def read_exact(stream: object, size: int, timeout: float) -> bytes:
    result: list[bytes] = []
    failure: list[BaseException] = []

    def read() -> None:
        try:
            result.append(stream.read(size))  # type: ignore[attr-defined]
        except BaseException as error:
            failure.append(error)

    worker = threading.Thread(target=read, daemon=True)
    worker.start()
    worker.join(timeout)
    if worker.is_alive():
        raise TimeoutError("Native Host did not return a framed response in time")
    if failure:
        raise failure[0]
    return result[0]


def native_message(process: subprocess.Popen[bytes], message: dict[str, object]) -> dict[str, object]:
    if process.stdin is None or process.stdout is None:
        raise RuntimeError("Native Host pipes are unavailable")
    process.stdin.write(frame(message))
    process.stdin.flush()
    header = read_exact(process.stdout, 4, 10)
    if len(header) != 4:
        raise RuntimeError("Native Host returned a truncated frame header")
    length = struct.unpack("<I", header)[0]
    body = read_exact(process.stdout, length, 10)
    if len(body) != length:
        raise RuntimeError("Native Host returned a truncated frame")
    result = json.loads(body.decode("utf-8"))
    if not isinstance(result, dict):
        raise RuntimeError("Native Host response was not an object")
    return result


def process_windows(pid: int) -> list[tuple[str, bool, tuple[int, int, int, int]]]:
    windows: list[tuple[str, bool, tuple[int, int, int, int]]] = []
    enum_proc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    @enum_proc
    def callback(hwnd: int, _lparam: int) -> bool:
        owner_pid = wintypes.DWORD()
        ctypes.windll.user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner_pid))
        if owner_pid.value != pid:
            return True
        length = ctypes.windll.user32.GetWindowTextLengthW(hwnd)
        buffer = ctypes.create_unicode_buffer(length + 1)
        ctypes.windll.user32.GetWindowTextW(hwnd, buffer, len(buffer))
        rect = wintypes.RECT()
        ctypes.windll.user32.GetWindowRect(hwnd, ctypes.byref(rect))
        windows.append((
            buffer.value,
            bool(ctypes.windll.user32.IsWindowVisible(hwnd)),
            (rect.left, rect.top, rect.right, rect.bottom),
        ))
        return True

    ctypes.windll.user32.EnumWindows(callback, 0)
    return windows


def visible_window(pid: int, title: str) -> bool:
    virtual_left = ctypes.windll.user32.GetSystemMetrics(76)
    virtual_top = ctypes.windll.user32.GetSystemMetrics(77)
    virtual_right = virtual_left + ctypes.windll.user32.GetSystemMetrics(78)
    virtual_bottom = virtual_top + ctypes.windll.user32.GetSystemMetrics(79)
    return any(
        window_title == title
        and visible
        and rect[2] > virtual_left
        and rect[0] < virtual_right
        and rect[3] > virtual_top
        and rect[1] < virtual_bottom
        for window_title, visible, rect in process_windows(pid)
    )


def wait_window(pid: int, title: str, expected: bool, timeout: float) -> float:
    started = time.perf_counter()
    while time.perf_counter() - started < timeout:
        if visible_window(pid, title) is expected:
            return (time.perf_counter() - started) * 1000
        time.sleep(0.002)
    state = "visible" if expected else "hidden"
    raise TimeoutError(
        f"Presenter window {title!r} did not become {state}; process windows={process_windows(pid)!r}"
    )


def percentile95(values: list[float]) -> float:
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * 0.95) - 1)]


def stop_process(process: subprocess.Popen[bytes] | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=3)


def visible_handoff_smoke(presenter_source: Path, host_source: Path, engine_source: Path) -> dict[str, object]:
    root = Path(tempfile.mkdtemp(prefix="hls-v7-presenter-visible-"))
    presenter = root / "HLSDownloaderPresenter.exe"
    host = root / "HLSDownloaderNativeHost.exe"
    engine = root / "HLSDownloaderEngine.exe"
    engine_process: subprocess.Popen[bytes] | None = None
    presenter_process: subprocess.Popen[bytes] | None = None
    host_process: subprocess.Popen[bytes] | None = None
    environment = os.environ.copy()
    environment["HLS_V7_DATA_DIR"] = str(root / "data")
    environment["HLS_V6_DATA_DIR"] = environment["HLS_V7_DATA_DIR"]
    environment["HLS_V7_PIPE"] = rf"\\.\pipe\HLSDownloader.v7-presenter-{uuid.uuid4().hex}"
    environment["HLS_V6_PIPE"] = environment["HLS_V7_PIPE"]
    environment["HLS_V7_PRESENTER_TRACE"] = "1"
    ready_file = root / "presenter.ready"
    environment["HLS_V7_PRESENTER_READY_FILE"] = str(ready_file)
    try:
        shutil.copy2(presenter_source, presenter)
        shutil.copy2(host_source, host)
        shutil.copy2(engine_source, engine)
        engine_process = subprocess.Popen([str(engine)], env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        presenter_process = subprocess.Popen(
            [str(presenter)], env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE
        )
        prewarm_started = time.perf_counter()
        while not ready_file.exists():
            if presenter_process.poll() is not None:
                raise RuntimeError(f"Presenter exited during renderer prewarm: {presenter_process.returncode}")
            if time.perf_counter() - prewarm_started > 5:
                raise TimeoutError("Presenter renderer did not report ready within 5 seconds")
            time.sleep(0.005)
        prewarm_ms = (time.perf_counter() - prewarm_started) * 1000
        host_process = subprocess.Popen(
            [str(host)], env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
        )
        ping = native_message(host_process, {"op": "ping"})
        if ping.get("ok") is not True:
            raise RuntimeError(f"Native Host did not connect to isolated Core: {ping}")
        if presenter_process.poll() is not None:
            raise RuntimeError(f"Presenter exited before the first browser offer: {presenter_process.returncode}")
        wait_window(presenter_process.pid, "确认下载", False, 2)
        latencies: list[float] = []
        submit_latencies: list[float] = []
        visibility_latencies: list[float] = []
        for index in range(20):
            if index == 10:
                stop_process(host_process)
                host_process = None
                stop_process(engine_process)
                engine_process = subprocess.Popen(
                    [str(engine)], env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
                )
                host_process = subprocess.Popen(
                    [str(host)], env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
                )
                restarted_ping = native_message(host_process, {"op": "ping"})
                if restarted_ping.get("ok") is not True:
                    raise RuntimeError(f"Native Host/Core restart did not reconnect: {restarted_ping}")
            started = time.perf_counter()
            response = native_message(host_process, {
                "op": "offer",
                "resource": {
                    "url": f"https://fixture.invalid/media-{index}.mp4",
                    "filename": f"media-{index}.mp4",
                    "title": f"Presenter fixture {index}",
                    "resource_kind": "file",
                    "mime_type": "video/mp4",
                    "size": 1_048_576,
                    "client_request_id": f"presenter-visible-{index}",
                },
            })
            submitted = time.perf_counter()
            handoff = response.get("handoff")
            if response.get("ok") is not True or not isinstance(handoff, dict):
                raise RuntimeError(f"Native Host offer failed: {response}")
            try:
                wait_window(presenter_process.pid, "确认下载", True, 2)
            except TimeoutError as error:
                exit_code = presenter_process.poll()
                if exit_code is None:
                    stop_process(presenter_process)
                    exit_code = presenter_process.returncode
                details = ""
                if presenter_process.stderr is not None:
                    details = presenter_process.stderr.read().decode("utf-8", errors="replace")
                raise TimeoutError(f"{error}; presenter_exit={exit_code}; stderr={details!r}") from error
            visible = time.perf_counter()
            latencies.append((visible - started) * 1000)
            submit_latencies.append((submitted - started) * 1000)
            visibility_latencies.append((visible - submitted) * 1000)
            handoff_id = handoff.get("id")
            rejected = native_message(host_process, {"op": "reject_handoff", "handoff_id": handoff_id})
            if rejected.get("ok") is not True:
                raise RuntimeError(f"Native Host reject failed: {rejected}")
            wait_window(presenter_process.pid, "确认下载", False, 2)
        p95 = percentile95(latencies)
        report = {
            "visible_offer_samples": len(latencies),
            "visible_offer_p95_ms": round(p95, 2),
            "visible_offer_max_ms": round(max(latencies), 2),
            "samples_ms": [round(value, 2) for value in latencies],
            "native_host_submit_p95_ms": round(percentile95(submit_latencies), 2),
            "native_host_submit_max_ms": round(max(submit_latencies), 2),
            "native_host_submit_samples_ms": [round(value, 2) for value in submit_latencies],
            "post_submit_visible_p95_ms": round(percentile95(visibility_latencies), 2),
            "post_submit_visible_max_ms": round(max(visibility_latencies), 2),
            "post_submit_visible_samples_ms": [round(value, 2) for value in visibility_latencies],
            "renderer_prewarm_ms": round(prewarm_ms, 2),
            "native_host_core_restart": True,
            "threshold_ms": 100,
            "passed": p95 <= 100,
        }
        if not report["passed"]:
            raise RuntimeError(f"Presenter visible offer P95 exceeded 100ms: {report}")
        return report
    finally:
        stop_process(host_process)
        stop_process(presenter_process)
        stop_process(engine_process)
        shutil.rmtree(root, ignore_errors=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--presenter", required=True, type=Path)
    parser.add_argument("--host", type=Path)
    parser.add_argument("--engine", type=Path)
    args = parser.parse_args()
    presenter = str(args.presenter.resolve())
    first = subprocess.Popen([presenter, "--lock-test"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        time.sleep(0.25)
        second = subprocess.run([presenter, "--lock-test"], capture_output=True, text=True, timeout=3)
        if second.returncode == 0 or "already running" not in (second.stdout + second.stderr):
            raise SystemExit(f"presenter election failed: exit={second.returncode} output={second.stdout}{second.stderr}")
        print('{"presenter_lock":"passed","second_exit":%d}' % second.returncode)
    finally:
        first.terminate()
        try:
            first.wait(timeout=2)
        except subprocess.TimeoutExpired:
            first.kill()
            first.wait(timeout=2)
    if (args.host is None) != (args.engine is None):
        raise SystemExit("--host and --engine must be supplied together")
    if args.host is not None and args.engine is not None:
        report = visible_handoff_smoke(args.presenter.resolve(), args.host.resolve(), args.engine.resolve())
        print(json.dumps(report, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
