from __future__ import annotations

import json
import os
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path


ROOT = Path(sys.executable).resolve().parent if getattr(sys, "frozen", False) else Path(__file__).resolve().parent.parent


def _settings() -> tuple[str, str]:
    local_app_data = os.environ.get("LOCALAPPDATA", "")
    candidates = [
        Path(local_app_data) / "HLS Downloader" / "config.json" if local_app_data else ROOT / ".missing",
        ROOT / "config.json",
        ROOT / "config.default.json",
    ]
    for path in candidates:
        if path.exists():
            data = json.loads(path.read_text(encoding="utf-8"))
            return f"http://127.0.0.1:{int(data.get('port', 8765))}/api", str(data.get("token", "55555"))
    return "http://127.0.0.1:8765/api", "55555"


def _request(method: str, path: str, payload: dict | None = None) -> dict | list:
    base, token = _settings()
    body = json.dumps(payload).encode("utf-8") if payload is not None else None
    request = urllib.request.Request(base + path, data=body, method=method)
    request.add_header("X-Token", token)
    request.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(request, timeout=4) as response:
        return json.loads(response.read().decode("utf-8"))


def _start_app() -> None:
    executable = ROOT / "HLSDownloader.exe"
    if executable.exists():
        subprocess.Popen(
            [str(executable), "--activate"],
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            creationflags=0x08000000,
        )


def _ensure_app() -> None:
    try:
        _request("GET", "/health")
        return
    except Exception:
        _start_app()
    for _ in range(80):
        time.sleep(0.25)
        try:
            _request("GET", "/health")
            return
        except Exception:
            pass
    raise RuntimeError("桌面下载器未启动或无法连接")


def dispatch(message: dict) -> dict:
    operation = message.get("op")
    if operation not in {"ping", "activate", "offer", "download", "handoff_status"}:
        raise ValueError("不支持的 Native Messaging 操作")
    _ensure_app()
    _request("POST", "/browser/ping", {"version": str(message.get("version", ""))})
    if operation == "ping":
        return {"ok": True, "version": _request("GET", "/health").get("version", "")}
    if operation == "activate":
        return {"ok": True, "result": _request("POST", "/app/activate", {})}
    if operation == "offer":
        return {"ok": True, "handoff": _request("POST", "/browser/handoffs", message.get("resource", {}))}
    if operation == "download":
        task = _request("POST", "/browser/downloads", message.get("resource", {}))
        activated = _request("POST", "/app/activate", {})
        return {"ok": True, "task": task, "activated": bool(activated.get("ok"))}
    handoff_id = str(message.get("handoff_id", ""))
    return {"ok": True, "handoff": _request("GET", f"/browser/handoffs/{handoff_id}")}


def _read_message() -> dict | None:
    raw = sys.stdin.buffer.read(4)
    if not raw:
        return None
    length = struct.unpack("<I", raw)[0]
    if length > 4 * 1024 * 1024:
        raise ValueError("Native Messaging 消息过大")
    return json.loads(sys.stdin.buffer.read(length).decode("utf-8"))


def _write_message(message: dict) -> None:
    raw = json.dumps(message, ensure_ascii=False).encode("utf-8")
    sys.stdout.buffer.write(struct.pack("<I", len(raw)))
    sys.stdout.buffer.write(raw)
    sys.stdout.buffer.flush()


def main() -> int:
    while True:
        try:
            message = _read_message()
            if message is None:
                return 0
            _write_message(dispatch(message))
        except Exception as exc:
            _write_message({"ok": False, "error": str(exc)})


if __name__ == "__main__":
    raise SystemExit(main())
