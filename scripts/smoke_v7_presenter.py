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
# 稳态门限的样本数：P95 只有在这个规模上才有意义（对小集合 P95 会退化为最大值，
# 单个调度抖动就会翻转判定）。暖机样本不计入该数量。
STEADY_STATE_SAMPLES = 20
# 采集原始 offer 的总次数上限（含暖机样本）——为暖机与崩溃恢复样本留出余量。
MAX_OFFER_ATTEMPTS = STEADY_STATE_SAMPLES + 8
# 首次渲染（第 1 个 offer）与每次崩溃恢复后的第 1 个 offer 属于一次性窗口首绘/重绘
# 开销，不代表稳态可见延迟，故不计入 100ms 稳态 P95 门限，单独记录以便观测。


def presenter_process_ids(suffix: str = "presenter.exe") -> list[int]:
    """Return the PIDs of running processes whose image name ends with `suffix`.

    A leftover presenter holds the session-global named mutex
    ``Local\\HLSDownloader.v7.presenter``. Any such instance makes a freshly
    launched presenter lose the election and exit with code 0 before it ever
    renders, which surfaces as a confusing "exited during renderer prewarm"
    failure that has nothing to do with the product under test.
    """

    class ProcessEntry32W(ctypes.Structure):
        _fields_ = [
            ("dwSize", wintypes.DWORD),
            ("cntUsage", wintypes.DWORD),
            ("th32ProcessID", wintypes.DWORD),
            ("th32DefaultHeapID", ctypes.POINTER(ctypes.c_ulong)),
            ("th32ModuleID", wintypes.DWORD),
            ("cntThreads", wintypes.DWORD),
            ("th32ParentProcessID", wintypes.DWORD),
            ("pcPriClassBase", ctypes.c_long),
            ("dwFlags", wintypes.DWORD),
            ("szExeFile", wintypes.WCHAR * 260),
        ]

    kernel32 = ctypes.windll.kernel32
    kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    kernel32.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    kernel32.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry32W)]
    kernel32.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry32W)]
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    snapshot = kernel32.CreateToolhelp32Snapshot(0x00000002, 0)  # TH32CS_SNAPPROCESS
    if not snapshot or snapshot == ctypes.c_void_p(-1).value:
        return []
    entry = ProcessEntry32W()
    entry.dwSize = ctypes.sizeof(ProcessEntry32W)
    pids: list[int] = []
    try:
        if not kernel32.Process32FirstW(snapshot, ctypes.byref(entry)):
            return []
        while True:
            if str(entry.szExeFile).lower().endswith(suffix):
                pids.append(int(entry.th32ProcessID))
            if not kernel32.Process32NextW(snapshot, ctypes.byref(entry)):
                break
    finally:
        kernel32.CloseHandle(snapshot)
    return pids


def stop_process(process: subprocess.Popen[bytes] | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=3)


def visible_handoff_smoke(
    presenter_source: Path,
    host_source: Path,
    engine_source: Path,
    require_latency: bool = True,
) -> dict[str, object]:
    root = Path(tempfile.mkdtemp(prefix="hls-v7-presenter-visible-"))
    presenter = root / "HLSDownloaderPresenter.exe"
    host = root / "HLSDownloaderNativeHost.exe"
    engine = root / "HLSDownloaderEngine.exe"
    engine_process: subprocess.Popen[bytes] | None = None
    presenter_process: subprocess.Popen[bytes] | None = None
    host_process: subprocess.Popen[bytes] | None = None
    environment = os.environ.copy()
    environment["HLS_V7_DATA_DIR"] = str(root / "data")
    environment["HLS_V7_PIPE"] = rf"\\.\pipe\HLSDownloader.v7-presenter-{uuid.uuid4().hex}"
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
                details = ""
                if presenter_process.stderr is not None:
                    details = presenter_process.stderr.read().decode("utf-8", errors="replace").strip()
                hint = ""
                if presenter_process.returncode == 0:
                    hint = (
                        " A clean exit here means the presenter lost the single-instance election: "
                        "another instance holds the session-global mutex "
                        "Local\\HLSDownloader.v7.presenter (any leftover copy, including one left "
                        "behind by an earlier packaged-app gate run, is enough)."
                    )
                raise RuntimeError(
                    f"Presenter exited during renderer prewarm: {presenter_process.returncode}"
                    f"; windows={process_windows(presenter_process.pid)!r}"
                    f"; stderr={details!r}.{hint}"
                )
            if time.perf_counter() - prewarm_started > 15:
                details = ""
                if presenter_process.stderr is not None and presenter_process.poll() is not None:
                    details = presenter_process.stderr.read().decode("utf-8", errors="replace")
                raise TimeoutError(
                    "Presenter renderer did not report ready within 15 seconds"
                    f"; presenter_exit={presenter_process.poll()}; engine_exit={engine_process.poll() if engine_process else None}"
                    f"; windows={process_windows(presenter_process.pid)!r}; root={root}; stderr={details!r}"
                )
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
        warmup_latencies: list[float] = []
        submit_latencies: list[float] = []
        visibility_latencies: list[float] = []
        warmup_submit_latencies: list[float] = []
        warmup_visibility_latencies: list[float] = []
        presenter_pid_before_crash = 0
        presenter_pid_after_restart = 0
        # 恢复点：第 5 次原始 offer 之后重启 presenter，第 10 次之后重启 host+core。
        # 这两处之后的第 1 个 offer 视为暖机样本（一次性重绘开销），不计入稳态集合。
        presenter_restart_after = 5
        core_restart_after = 10
        expect_warmup_next = True
        index = -1
        while len(latencies) < STEADY_STATE_SAMPLES and index + 1 < MAX_OFFER_ATTEMPTS:
            index += 1
            if index == core_restart_after:
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
            sample_ms = (visible - started) * 1000
            # 暖机样本：presenter 首次渲染（第 1 个 offer）以及每次崩溃恢复后的第 1 个 offer
            # 含一次性窗口首绘/重绘开销，不代表稳态可见延迟，故不计入 100ms 稳态 P95 门限，
            # 也不计入各分段的稳态百分位（否则一次性暖机离群值会污染 native_host_submit /
            # post_submit_visible 这两个本应描述稳态的指标）；单独记录以便观测。
            # 其余样本计入稳态集合，直到收满 STEADY_STATE_SAMPLES 个。
            submit_ms = (submitted - started) * 1000
            visibility_ms = (visible - submitted) * 1000
            if expect_warmup_next:
                warmup_latencies.append(sample_ms)
                warmup_submit_latencies.append(submit_ms)
                warmup_visibility_latencies.append(visibility_ms)
                expect_warmup_next = False
            else:
                latencies.append(sample_ms)
                submit_latencies.append(submit_ms)
                visibility_latencies.append(visibility_ms)
            handoff_id = handoff.get("id")
            if index == presenter_restart_after:
                presenter_pid_before_crash = presenter_process.pid
                stop_process(presenter_process)
                ready_file.unlink(missing_ok=True)
                presenter_process = subprocess.Popen(
                    [str(presenter)], env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE
                )
                presenter_pid_after_restart = presenter_process.pid
                restart_deadline = time.monotonic() + 30
                while not ready_file.exists():
                    if presenter_process.poll() is not None:
                        raise RuntimeError(f"Presenter exited during recovery: {presenter_process.returncode}")
                    if time.monotonic() >= restart_deadline:
                        raise TimeoutError("restarted Presenter did not report ready within 30 seconds")
                    time.sleep(0.01)
                wait_window(presenter_process.pid, "确认下载", True, 30)
                expect_warmup_next = True
            rejected = native_message(host_process, {"op": "reject_handoff", "handoff_id": handoff_id})
            if rejected.get("ok") is not True:
                raise RuntimeError(f"Native Host reject failed: {rejected}")
            wait_window(presenter_process.pid, "确认下载", False, 2)
        if not latencies:
            raise RuntimeError("Presenter smoke produced no steady-state latency samples")
        p95 = percentile95(latencies)
        report = {
            "visible_offer_samples": len(latencies) + len(warmup_latencies),
            "steady_state_samples": len(latencies),
            "visible_offer_p95_ms": round(p95, 2),
            "visible_offer_max_ms": round(max(latencies), 2),
            "warmup_submit_max_ms": round(max(warmup_submit_latencies), 2) if warmup_submit_latencies else 0.0,
            "warmup_post_submit_visible_max_ms": round(max(warmup_visibility_latencies), 2)
            if warmup_visibility_latencies
            else 0.0,
            "samples_ms": [round(value, 2) for value in latencies],
            "warmup_samples_ms": [round(value, 2) for value in warmup_latencies],
            "warmup_max_ms": round(max(warmup_latencies), 2) if warmup_latencies else 0.0,
            "native_host_submit_p95_ms": round(percentile95(submit_latencies), 2),
            "native_host_submit_max_ms": round(max(submit_latencies), 2),
            "native_host_submit_samples_ms": [round(value, 2) for value in submit_latencies],
            "post_submit_visible_p95_ms": round(percentile95(visibility_latencies), 2),
            "post_submit_visible_max_ms": round(max(visibility_latencies), 2),
            "post_submit_visible_samples_ms": [round(value, 2) for value in visibility_latencies],
            "renderer_prewarm_ms": round(prewarm_ms, 2),
            "native_host_core_restart": True,
            "presenter_pid_before_crash": presenter_pid_before_crash,
            "presenter_pid_after_restart": presenter_pid_after_restart,
            "presenter_pending_recovery": presenter_pid_before_crash > 0 and presenter_pid_after_restart > 0,
            "threshold_ms": 100,
            "latency_passed": p95 <= 100,
            "passed": presenter_pid_before_crash > 0 and presenter_pid_after_restart > 0,
        }
        if require_latency and not report["latency_passed"]:
            raise RuntimeError(
                f"Presenter steady-state visible offer P95 exceeded 100ms: {report}"
            )
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
    parser.add_argument("--recovery-only", action="store_true")
    args = parser.parse_args()
    stray = presenter_process_ids()
    if stray:
        raise SystemExit(
            "another presenter instance is already running "
            f"(pid={', '.join(str(pid) for pid in stray)}); the presenter holds the "
            "session-global mutex Local\\HLSDownloader.v7.presenter, so the instance launched "
            "below would exit with code 0 before rendering and every assertion in this smoke "
            "would fail for the wrong reason. Stop the stray instance first: "
            "Get-Process HLSDownloaderPresenter | Stop-Process -Force"
        )
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
        report = visible_handoff_smoke(
            args.presenter.resolve(), args.host.resolve(), args.engine.resolve(), not args.recovery_only
        )
        print(json.dumps(report, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
