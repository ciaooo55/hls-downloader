from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import json
import math
import os
from pathlib import Path
import shutil
import socket
import statistics
import struct
import subprocess
import tempfile
import time


PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010


class ProcessMemoryCountersEx(ctypes.Structure):
    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
        ("PrivateUsage", ctypes.c_size_t),
    ]


def filetime_value(value: wintypes.FILETIME) -> int:
    return (int(value.dwHighDateTime) << 32) | int(value.dwLowDateTime)


class ProcessProbe:
    def __init__(self, pid: int) -> None:
        self.kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        self.psapi = ctypes.WinDLL("psapi", use_last_error=True)
        self.handle = self.kernel32.OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, pid
        )
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())

    def close(self) -> None:
        if self.handle:
            self.kernel32.CloseHandle(self.handle)
            self.handle = None

    def sample(self, elapsed_seconds: float) -> dict[str, float | int]:
        memory = ProcessMemoryCountersEx()
        memory.cb = ctypes.sizeof(memory)
        if not self.psapi.GetProcessMemoryInfo(
            self.handle, ctypes.byref(memory), memory.cb
        ):
            raise ctypes.WinError(ctypes.get_last_error())

        handles = wintypes.DWORD()
        if not self.kernel32.GetProcessHandleCount(self.handle, ctypes.byref(handles)):
            raise ctypes.WinError(ctypes.get_last_error())

        creation = wintypes.FILETIME()
        exit_time = wintypes.FILETIME()
        kernel = wintypes.FILETIME()
        user = wintypes.FILETIME()
        if not self.kernel32.GetProcessTimes(
            self.handle,
            ctypes.byref(creation),
            ctypes.byref(exit_time),
            ctypes.byref(kernel),
            ctypes.byref(user),
        ):
            raise ctypes.WinError(ctypes.get_last_error())

        return {
            "elapsed_seconds": round(elapsed_seconds, 3),
            "working_set_mib": round(memory.WorkingSetSize / 1024 / 1024, 3),
            "private_mib": round(memory.PrivateUsage / 1024 / 1024, 3),
            "handles": int(handles.value),
            "cpu_seconds": round(
                (filetime_value(kernel) + filetime_value(user)) / 10_000_000, 6
            ),
        }

    def __enter__(self) -> ProcessProbe:
        return self

    def __exit__(self, *_args: object) -> None:
        self.close()


def frame(message: dict[str, object]) -> bytes:
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    return struct.pack("<I", len(payload)) + payload


def read_exact(stream: socket.socket, size: int) -> bytes:
    result = bytearray()
    while len(result) < size:
        chunk = stream.recv(size - len(result))
        if not chunk:
            raise RuntimeError("Core IPC closed before the response completed")
        result.extend(chunk)
    return bytes(result)


def send_frame(stream: socket.socket, message: dict[str, object]) -> dict[str, object]:
    stream.sendall(frame(message))
    size = struct.unpack("<I", read_exact(stream, 4))[0]
    response = json.loads(read_exact(stream, size).decode("utf-8"))
    if not isinstance(response, dict):
        raise RuntimeError("Core IPC response was not an object")
    return response


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def wait_for_core(port: int, deadline: float) -> socket.socket:
    while time.monotonic() < deadline:
        try:
            stream = socket.create_connection(("127.0.0.1", port), timeout=0.2)
            stream.settimeout(5)
            response = send_frame(
                stream,
                {"type": "hello", "protocol": "hls-downloader-v7-core", "version": 1},
            )
            if response.get("type") == "hello":
                return stream
            stream.close()
        except OSError:
            time.sleep(0.02)
    raise TimeoutError("isolated Engine did not expose the Core IPC endpoint")


def stop_process(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=3)


def cpu_percent(samples: list[dict[str, float | int]]) -> float:
    if len(samples) < 2:
        return 0.0
    cpu_delta = float(samples[-1]["cpu_seconds"]) - float(samples[0]["cpu_seconds"])
    wall_delta = float(samples[-1]["elapsed_seconds"]) - float(samples[0]["elapsed_seconds"])
    return max(0.0, cpu_delta / max(wall_delta, 0.001) * 100)


def delta(samples: list[dict[str, float | int]], field: str) -> float:
    return float(samples[-1][field]) - float(samples[0][field])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    parser.add_argument("--idle-seconds", type=int, default=300)
    parser.add_argument("--stress-requests", type=int, default=18_000)
    parser.add_argument("--equivalent-request-rate", type=float, default=10.0)
    args = parser.parse_args()

    if os.name != "nt":
        raise RuntimeError("this soak fixture measures a Windows Engine process")
    if not args.engine.is_file():
        raise FileNotFoundError(args.engine)
    if args.idle_seconds < 30 or args.stress_requests < 100:
        raise ValueError("soak test requires at least 30 idle seconds and 100 requests")

    root = Path(tempfile.mkdtemp(prefix="hls-v7-runtime-soak-"))
    engine_path = root / "HLSDownloaderEngine.exe"
    shutil.copy2(args.engine, engine_path)
    port = free_port()
    environment = os.environ.copy()
    environment.update(
        {
            "HLS_V7_DATA_DIR": str(root / "data"),
            "HLS_V7_DOWNLOAD_DIR": str(root / "downloads"),
            "HLS_V7_CORE_TCP": "1",
            "HLS_V7_CORE_BIND": f"127.0.0.1:{port}",
            "HLS_V6_SKIP_MIGRATE": "1",
        }
    )
    engine = subprocess.Popen(
        [str(engine_path)],
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    stream: socket.socket | None = None
    started = time.monotonic()
    try:
        stream = wait_for_core(port, time.monotonic() + 10)
        request_id = 1
        for _ in range(100):
            response = send_frame(stream, {"type": "snapshot", "request_id": request_id})
            request_id += 1
            if response.get("type") == "error":
                raise RuntimeError(f"Core rejected warmup snapshot: {response}")

        with ProcessProbe(engine.pid) as probe:
            idle_samples = [probe.sample(time.monotonic() - started)]
            idle_deadline = time.monotonic() + args.idle_seconds
            while time.monotonic() < idle_deadline:
                time.sleep(min(1.0, max(0.0, idle_deadline - time.monotonic())))
                if engine.poll() is not None:
                    raise RuntimeError(f"Engine exited during idle soak: {engine.returncode}")
                idle_samples.append(probe.sample(time.monotonic() - started))

            stress_samples = [probe.sample(time.monotonic() - started)]
            latencies: list[float] = []
            errors: list[str] = []
            stress_started = time.monotonic()
            last_sample = stress_started
            for _ in range(args.stress_requests):
                call_started = time.perf_counter()
                response = send_frame(
                    stream, {"type": "snapshot", "request_id": request_id}
                )
                latencies.append((time.perf_counter() - call_started) * 1000)
                request_id += 1
                if response.get("type") == "error":
                    errors.append(json.dumps(response, ensure_ascii=False))
                if time.monotonic() - last_sample >= 0.25:
                    stress_samples.append(probe.sample(time.monotonic() - started))
                    last_sample = time.monotonic()
            stress_samples.append(probe.sample(time.monotonic() - started))
            stress_duration = time.monotonic() - stress_started

            time.sleep(5)
            settled = probe.sample(time.monotonic() - started)

        idle_ws_growth = delta(idle_samples, "working_set_mib")
        idle_private_growth = delta(idle_samples, "private_mib")
        idle_handle_growth = int(delta(idle_samples, "handles"))
        stress_ws_growth = delta(stress_samples, "working_set_mib")
        stress_private_growth = delta(stress_samples, "private_mib")
        stress_handle_growth = int(delta(stress_samples, "handles"))
        ipc_p95 = percentile(latencies, 0.95)
        ipc_p99 = percentile(latencies, 0.99)
        equivalent_seconds = args.stress_requests / args.equivalent_request_rate

        thresholds = {
            "idle_working_set_growth_mib": 8,
            "idle_private_growth_mib": 8,
            "idle_handle_growth": 4,
            "idle_cpu_percent": 2,
            "stress_working_set_growth_mib": 16,
            "stress_private_growth_mib": 16,
            "stress_handle_growth": 4,
            "ipc_p95_ms": 75,
            "ipc_max_ms": 500,
            "errors": 0,
        }
        passed = (
            idle_ws_growth <= thresholds["idle_working_set_growth_mib"]
            and idle_private_growth <= thresholds["idle_private_growth_mib"]
            and idle_handle_growth <= thresholds["idle_handle_growth"]
            and cpu_percent(idle_samples) <= thresholds["idle_cpu_percent"]
            and stress_ws_growth <= thresholds["stress_working_set_growth_mib"]
            and stress_private_growth <= thresholds["stress_private_growth_mib"]
            and stress_handle_growth <= thresholds["stress_handle_growth"]
            and ipc_p95 <= thresholds["ipc_p95_ms"]
            and max(latencies) <= thresholds["ipc_max_ms"]
            and not errors
            and engine.poll() is None
        )
        report = {
            "schema": 1,
            "engine": str(args.engine.resolve()),
            "idle": {
                "actual_seconds": args.idle_seconds,
                "samples": len(idle_samples),
                "working_set_start_mib": idle_samples[0]["working_set_mib"],
                "working_set_end_mib": idle_samples[-1]["working_set_mib"],
                "working_set_growth_mib": round(idle_ws_growth, 3),
                "private_start_mib": idle_samples[0]["private_mib"],
                "private_end_mib": idle_samples[-1]["private_mib"],
                "private_growth_mib": round(idle_private_growth, 3),
                "handles_start": idle_samples[0]["handles"],
                "handles_end": idle_samples[-1]["handles"],
                "handle_growth": idle_handle_growth,
                "cpu_percent": round(cpu_percent(idle_samples), 3),
            },
            "stress": {
                "request_count": len(latencies),
                "actual_seconds": round(stress_duration, 3),
                "equivalent_seconds_at_requested_rate": round(equivalent_seconds, 3),
                "throughput_requests_per_second": round(len(latencies) / stress_duration, 2),
                "ipc_mean_ms": round(statistics.fmean(latencies), 3),
                "ipc_p95_ms": round(ipc_p95, 3),
                "ipc_p99_ms": round(ipc_p99, 3),
                "ipc_max_ms": round(max(latencies), 3),
                "working_set_growth_mib": round(stress_ws_growth, 3),
                "private_growth_mib": round(stress_private_growth, 3),
                "handle_growth": stress_handle_growth,
                "cpu_percent": round(cpu_percent(stress_samples), 3),
                "errors": errors,
            },
            "settled": settled,
            "thresholds": thresholds,
            "engine_alive_at_end": engine.poll() is None,
            "passed": passed,
        }
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(
            json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8"
        )
        print(json.dumps(report, ensure_ascii=False))
        if not passed:
            raise RuntimeError(f"v7 runtime soak thresholds failed: {report}")
        return 0
    finally:
        if stream is not None:
            stream.close()
        stop_process(engine)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
