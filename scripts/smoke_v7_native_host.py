from __future__ import annotations

import argparse
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


def responses(executable: Path, environment: dict[str, str]) -> tuple[list[dict[str, object]], float, float]:
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
        return result, first_response_ms, (time.perf_counter() - started) * 1000
    finally:
        process.stdin.close()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=5)


def stop_isolated_engine(executable: Path) -> None:
    command = (
        "Get-CimInstance Win32_Process -Filter \"Name='HLSDownloaderEngine.exe'\" | "
        "Where-Object { $_.ExecutablePath -eq $env:HLS_V7_SMOKE_ENGINE } | "
        "ForEach-Object { Stop-Process -Id $_.ProcessId }; "
        "Start-Sleep -Milliseconds 150; "
        "$remaining = Get-CimInstance Win32_Process -Filter \"Name='HLSDownloaderEngine.exe'\" | "
        "Where-Object { $_.ExecutablePath -eq $env:HLS_V7_SMOKE_ENGINE }; "
        "if ($remaining) { exit 1 }"
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
    args = parser.parse_args()
    if not args.host.is_file() or not args.engine.is_file():
        raise RuntimeError("Native Host and engine binaries must exist before smoke testing")

    root = Path(tempfile.mkdtemp(prefix="hls-v7-native-host-"))
    engine = root / "HLSDownloaderEngine.exe"
    try:
        host = root / "HLSDownloaderNativeHost.exe"
        shutil.copy2(args.host, host)
        shutil.copy2(args.engine, engine)
        environment = os.environ.copy()
        environment["HLS_V7_DATA_DIR"] = str(root / "data")
        # The persistence module is shared with the frozen schema module while
        # its public runtime identity is v7; set the legacy storage alias only
        # for this isolated smoke fixture so the test does not touch user data.
        environment["HLS_V6_DATA_DIR"] = str(root / "data")
        environment["HLS_V7_PIPE"] = rf"\\.\pipe\HLSDownloader.v7-smoke-{uuid.uuid4().hex}"
        environment["HLS_V6_PIPE"] = environment["HLS_V7_PIPE"]
        received, first_response_ms, two_response_ms = responses(host, environment)
        if len(received) != 2 or any(item.get("ok") is not True for item in received):
            raise RuntimeError(f"Native Host ping contract failed: {received}")
        if any(item.get("protocol_version") != 1 for item in received):
            raise RuntimeError(f"Native Host protocol version changed: {received}")
        if not (root / "data" / "data.db").is_file():
            raise RuntimeError("Cold-started engine did not create its isolated database")
        if first_response_ms > 1500:
            raise RuntimeError(f"Cold Native Host/Core first response exceeded 1500ms: {first_response_ms:.2f}ms")
        report = {
            "schema": 1,
            "cold_first_response_ms": round(first_response_ms, 2),
            "two_response_total_ms": round(two_response_ms, 2),
            "threshold_ms": 1500,
            "passed": True,
        }
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(report, ensure_ascii=False))
        print("v7 Native Host cold-start smoke passed: two framed pings, isolated Core, clean exit")
        return 0
    finally:
        stop_isolated_engine(engine)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
