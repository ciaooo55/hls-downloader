from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import threading
import time
import uuid


def frame(message: dict[str, object]) -> bytes:
    payload = json.dumps(message, separators=(",", ":")).encode("utf-8")
    return struct.pack("<I", len(payload)) + payload


def read_exact(stream: object, size: int, timeout: float) -> bytes:
    result: list[bytes] = []
    failure: list[BaseException] = []

    def read() -> None:
        try:
            result.append(stream.read(size))  # type: ignore[attr-defined]
        except BaseException as error:  # Reader errors are reported on the test thread.
            failure.append(error)

    worker = threading.Thread(target=read, daemon=True)
    worker.start()
    worker.join(timeout)
    if worker.is_alive():
        raise TimeoutError("Native Host did not return a framed response in time")
    if failure:
        raise failure[0]
    return result[0]


def responses(executable: Path, environment: dict[str, str]) -> tuple[list[dict[str, object]], float, float, list[dict[str, object]]]:
    started = time.perf_counter()
    process = subprocess.Popen(
        [str(executable)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
    )
    assert process.stdin is not None
    assert process.stdout is not None
    assert process.stderr is not None
    diagnostics: list[dict[str, object]] = []

    def collect_diagnostics() -> None:
        for line in process.stderr:
            diagnostics.append({
                "received_ms": round((time.perf_counter() - started) * 1000, 2),
                "message": line.decode("utf-8", errors="replace").strip(),
            })

    threading.Thread(target=collect_diagnostics, daemon=True).start()
    try:
        payload = frame({"op": "ping"}) + frame({"op": "ping"})
        process.stdin.write(payload)
        process.stdin.flush()
        result: list[dict[str, object]] = []
        first_response_ms = 0.0
        for _ in range(2):
            header = read_exact(process.stdout, 4, 20)
            if len(header) != 4:
                raise RuntimeError("Native Host returned a truncated frame header")
            length = struct.unpack("<I", header)[0]
            body = read_exact(process.stdout, length, 10)
            if len(body) != length:
                raise RuntimeError("Native Host returned a truncated frame")
            parsed = json.loads(body.decode("utf-8"))
            if not isinstance(parsed, dict):
                raise RuntimeError("Native Host response was not an object")
            result.append(parsed)
            if len(result) == 1:
                first_response_ms = (time.perf_counter() - started) * 1000
        return result, first_response_ms, (time.perf_counter() - started) * 1000, list(diagnostics)
    except Exception as error:
        raise RuntimeError(f"Native Host response failed: {error}; startup diagnostics: {diagnostics}") from error
    finally:
        process.stdin.close()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=5)


def stop_isolated_engine(executable: Path) -> None:
    command = (
        "$ErrorActionPreference = 'Stop'; "
        "Get-Process -Name HLSDownloaderEngine -ErrorAction SilentlyContinue | "
        "Where-Object { $_.Path -eq $env:HLS_V7_SMOKE_ENGINE } | "
        "ForEach-Object { Stop-Process -InputObject $_ -PassThru | Wait-Process -Timeout 5 }"
    )
    environment = os.environ.copy()
    environment["HLS_V7_SMOKE_ENGINE"] = str(executable)
    completed = subprocess.run(
        ["powershell.exe", "-NoProfile", "-Command", command],
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
        env=environment,
    )
    if completed.returncode:
        raise RuntimeError(completed.stderr.strip() or "Could not stop isolated engine")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", required=True, type=Path)
    parser.add_argument("--engine", required=True, type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--stage-root", type=Path, default=None)
    parser.add_argument("--profile-startup", action="store_true")
    parser.add_argument("--concurrent-hosts", type=int, choices=range(1, 9), default=1)
    args = parser.parse_args()
    if not args.host.is_file() or not args.engine.is_file():
        raise RuntimeError("Native Host and engine binaries must exist before smoke testing")

    # 隔离 stage 目录的卷决定了“首次执行全新路径”的系统开销（杀毒扫描/预取缺失）。
    # 默认沿用系统临时目录；调用方可用 --stage-root 显式指定，并把卷记入报告。
    stage_root = Path(args.stage_root) if args.stage_root else Path(tempfile.gettempdir())
    root = Path(tempfile.mkdtemp(prefix="hls-v7-native-host-", dir=str(stage_root)))
    # The Native Host resolves its Core by the installed product name next to its
    # own executable, so stage both binaries under product names in an isolated
    # directory instead of depending on the caller's build layout.
    host = root / "HLSDownloaderNativeHost.exe"
    engine = root / "HLSDownloaderEngine.exe"
    try:
        shutil.copy2(args.host.resolve(), host)
        shutil.copy2(args.engine.resolve(), engine)
        # 夹具先落盘，以免将复制文件的延迟写入计入已安装程序的启动时间。
        for binary in (host, engine):
            with binary.open("rb+") as staged:
                staged.flush()
                os.fsync(staged.fileno())
        environment = os.environ.copy()
        environment["HLS_V7_DATA_DIR"] = str(root / "data")
        environment["HLS_V7_PIPE"] = rf"\\.\pipe\HLSDownloader.v7-smoke-{uuid.uuid4().hex}"
        if args.profile_startup:
            environment["HLS_V7_STARTUP_PROFILE"] = "1"
        barrier = threading.Barrier(args.concurrent_hosts)

        def start_host() -> tuple[list[dict[str, object]], float, float, list[dict[str, object]]]:
            barrier.wait(timeout=10)
            return responses(host, environment)

        with ThreadPoolExecutor(max_workers=args.concurrent_hosts) as executor:
            results = list(executor.map(lambda _: start_host(), range(args.concurrent_hosts)))
        for received, _, _, _ in results:
            if len(received) != 2 or any(item.get("ok") is not True for item in received):
                raise RuntimeError(f"Native Host ping contract failed: {received}")
            if any(item.get("protocol_version") != 1 for item in received):
                raise RuntimeError(f"Native Host protocol version changed: {received}")
        first_response_ms = max(result[1] for result in results)
        two_response_ms = max(result[2] for result in results)
        diagnostics = results[0][3]
        spawn_attempts = sum(
            "core_spawn stage=before_spawn" in str(item["message"])
            for result in results for item in result[3]
        )
        process_environment = environment.copy()
        process_environment["HLS_V7_SMOKE_ENGINE"] = str(engine)
        count = subprocess.run(
            ["powershell.exe", "-NoProfile", "-Command",
             "@(Get-CimInstance Win32_Process -Filter \"Name='HLSDownloaderEngine.exe'\" | "
             "Where-Object { $_.ExecutablePath -eq $env:HLS_V7_SMOKE_ENGINE }).Count"],
            capture_output=True, text=True, timeout=10, check=True, env=process_environment,
        )
        engine_count = int(count.stdout.strip())
        if engine_count != 1:
            raise RuntimeError(f"Expected one resident Core after host startup, got {engine_count}")
        if not (root / "data" / "data.db").is_file():
            raise RuntimeError("Cold-started engine did not create its isolated database")
        report = {
            "schema": 1,
            "host_sha256": hashlib.sha256(host.read_bytes()).hexdigest(),
            "engine_sha256": hashlib.sha256(engine.read_bytes()).hexdigest(),
            "staged_binaries_flushed": True,
            "stage_volume": str(root.resolve().drive).upper(),
            "stage_root": str(stage_root),
            "cold_first_response_ms": round(first_response_ms, 2),
            "two_response_total_ms": round(two_response_ms, 2),
            "threshold_ms": 1500,
            "passed": first_response_ms <= 1500 and spawn_attempts <= 1,
            "migration_skipped": bool(environment.get("HLS_V6_SKIP_MIGRATE")),
            "diagnostics": diagnostics,
            "diagnostics_by_host": [result[3] for result in results],
            "core_spawn_attempts": spawn_attempts,
            "concurrent_hosts": args.concurrent_hosts,
            "resident_engine_count": engine_count,
            "host_first_response_ms": [round(result[1], 2) for result in results],
        }
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(report, ensure_ascii=False))
        if spawn_attempts > 1:
            raise RuntimeError(f"Concurrent browser hosts launched {spawn_attempts} Core processes")
        if not report["passed"]:
            raise RuntimeError(f"Cold Native Host/Core first response exceeded 1500ms: {first_response_ms:.2f}ms")
        print("v7 Native Host cold-start smoke passed: two framed pings, isolated Core, clean exit")
        return 0
    finally:
        stop_isolated_engine(engine)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
