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


def _frozen_install_root(executable: Path) -> Path:
    """Resolve the app root for legacy and versioned Native Host layouts."""
    host_dir = executable.resolve().parent
    for candidate in (host_dir, host_dir.parent, host_dir.parent.parent):
        if (candidate / "HLSDownloader.exe").is_file() or (candidate / "portable").is_file():
            return candidate
    # Keep the legacy behavior when a partial install is being repaired.
    return host_dir


ROOT = (
    _frozen_install_root(Path(sys.executable))
    if getattr(sys, "frozen", False)
    else Path(__file__).resolve().parent.parent
)


def _settings() -> tuple[str, str]:
    local_app_data = os.environ.get("LOCALAPPDATA", "")
    if (ROOT / "portable").is_file():
        candidates = [ROOT / "config.json"]
    else:
        candidates = [
            Path(local_app_data) / "HLS Downloader" / "config.json" if local_app_data else ROOT / ".missing",
            ROOT / "config.json",
        ]
    for path in candidates:
        if path.exists():
            data = json.loads(path.read_text(encoding="utf-8"))
            return f"http://127.0.0.1:{int(data.get('port', 8765))}/api", str(data.get("token", ""))
    # A cold launch reaches this branch before the desktop core creates its
    # config. Every retry re-reads the file and picks up the generated secret.
    return "http://127.0.0.1:8765/api", ""


def _open_local(request: urllib.request.Request, timeout: float):
    """Connect to the loopback Core without inheriting a system web proxy."""
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    return opener.open(request, timeout=timeout)


def _request(method: str, path: str, payload: dict | None = None, timeout: float = 4) -> dict:
    base, token = _settings()
    body = json.dumps(payload).encode("utf-8") if payload is not None else None
    request = urllib.request.Request(base + path, data=body, method=method)
    request.add_header("X-Token", token)
    request.add_header("Content-Type", "application/json")
    try:
        with _open_local(request, timeout=timeout) as response:
            decoded = json.loads(response.read().decode("utf-8"))
            if not isinstance(decoded, dict):
                raise RuntimeError("桌面端返回了无效的对象响应")
            return decoded
    except urllib.error.HTTPError as exc:
        # Surface FastAPI's localized detail to the extension instead of the
        # unhelpful generic ``HTTP Error 502`` string.
        try:
            payload = json.loads(exc.read().decode("utf-8"))
            detail = payload.get("detail") if isinstance(payload, dict) else None
        except (ValueError, OSError):
            detail = None
        raise RuntimeError(str(detail or f"HTTP {exc.code}")) from exc


def _start_app() -> None:
    executable = ROOT / "HLSDownloader.exe"
    if executable.exists():
        subprocess.Popen(
            [str(executable), "--background"],
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            creationflags=0x08000000,
        )


def _wait_presenter(timeout: float = 18.0) -> None:
    """Wait until the desktop shell can queue or show handoff windows."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            status = _request("GET", "/browser/presenter")
            # A registered desktop session can durably queue the handoff even
            # before its presenter callback has attached. Waiting for `ready`
            # here made a healthy cold start consume the full 18-second window.
            if status.get("ready") or status.get("session"):
                return
        except Exception:
            pass
        time.sleep(0.12)


def _ensure_app(require_presenter: bool = True) -> None:
    started = False
    try:
        _request("GET", "/health")
    except Exception:
        _start_app()
        started = True
        if require_presenter:
            # `/browser/presenter` is also a readiness probe. Poll it directly
            # instead of first spending up to 12 seconds on `/health` and then
            # starting another independent 18-second wait.
            _wait_presenter(18.0)
            return
        for _ in range(80):
            time.sleep(0.15)
            try:
                _request("GET", "/health")
                break
            except Exception:
                pass
        else:
            raise RuntimeError("桌面下载器未启动或无法连接")
    if require_presenter:
        # Cold start: health is live before the desktop shell registers its
        # session, so wait before accepting operations that need desktop UI.
        _wait_presenter(18.0 if started else 2.5)


def dispatch(message: dict) -> dict:
    operation = message.get("op")
    if operation not in {
        "ping", "activate", "offer", "download", "handoff_status", "wait_handoff",
        "set_takeover_settings", "push_to_tv", "media_push", "media_push_status",
    }:
        raise ValueError("不支持的 Native Messaging 操作")
    _ensure_app(operation in {"offer", "media_push"})
    if operation == "ping":
        browser_status = _request(
            "POST",
            "/browser/ping",
            {
                "version": str(message.get("version", "")),
                "client_id": str(message.get("client_id", "")),
                "browser": str(message.get("browser", "")),
            },
        )
        health = _request("GET", "/health")
        current = _request("GET", "/settings")
        bridge_base, _control_token = _settings()
        browser_credential = _request("POST", "/browser/credential", {})
        return {
            "ok": True,
            "version": health.get("version", ""),
            # Native Messaging is the trusted pairing/bootstrap channel. The
            # extension then uses loopback HTTP for concurrent heartbeats and
            # handoffs, falling back here if the core restarts.
            "bridge_base": bridge_base,
            "bridge_token": str(browser_credential.get("credential", "")),
            "takeover_enabled": bool(current.get("browser_takeover_enabled", True)),
            "takeover_minimum_bytes": max(0, int(current.get("browser_takeover_min_mb", 0) or 0)) * 1024 * 1024,
            "recommended_extension_version": str(browser_status.get("recommended_version", "")),
            "minimum_extension_version": str(browser_status.get("minimum_version", "")),
            "extension_release_url": str(browser_status.get("release_url", "")),
        }
    # Heartbeats already POST /browser/ping. Interactive offers must not wait
    # on another round-trip before the desktop can show the confirmation window.
    if operation == "set_takeover_settings":
        payload: dict[str, object] = {}
        if "enabled" in message:
            payload["browser_takeover_enabled"] = bool(message["enabled"])
        if "minimum_bytes" in message:
            payload["browser_takeover_min_mb"] = max(0, int(message["minimum_bytes"] or 0)) // (1024 * 1024)
        current = _request("POST", "/settings", payload)
        return {
            "ok": True,
            "takeover_enabled": bool(current.get("browser_takeover_enabled", True)),
            "takeover_minimum_bytes": max(0, int(current.get("browser_takeover_min_mb", 0) or 0)) * 1024 * 1024,
        }
    if operation == "activate":
        return {"ok": True, "result": _request("POST", "/app/activate", {})}
    if operation == "offer":
        return {"ok": True, "handoff": _request("POST", "/browser/handoffs", message.get("resource", {}))}
    if operation == "download":
        task = _request("POST", "/browser/downloads", message.get("resource", {}))
        return {"ok": True, "task": task, "activated": False}
    if operation == "push_to_tv":
        return _request("POST", "/tvbox/push", {"url": str(message.get("resource", {}).get("url", ""))})
    if operation == "media_push":
        return _request("POST", "/browser/media-push", {
            "kind": str(message.get("kind", "")),
            "resource": message.get("resource", {}),
        })
    if operation == "media_push_status":
        request_id = str(message.get("request_id", ""))
        return _request("GET", f"/browser/media-push/{request_id}/status")
    handoff_id = str(message.get("handoff_id", ""))
    if operation == "wait_handoff":
        return {"ok": True, "handoff": _request("GET", f"/browser/handoffs/{handoff_id}/wait", timeout=125)}
    return {"ok": True, "handoff": _request("GET", f"/browser/handoffs/{handoff_id}")}


def _read_exact(stream, length: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < length:
        piece = stream.read(length - len(chunks))
        if not piece:
            raise EOFError("Native Messaging 消息被截断")
        chunks.extend(piece)
    return bytes(chunks)


def _read_message() -> dict | None:
    raw = sys.stdin.buffer.read(4)
    if not raw:
        return None
    if len(raw) != 4:
        raw += _read_exact(sys.stdin.buffer, 4 - len(raw))
    length = struct.unpack("<I", raw)[0]
    if length > 4 * 1024 * 1024:
        raise ValueError("Native Messaging 消息过大")
    return json.loads(_read_exact(sys.stdin.buffer, length).decode("utf-8"))


def _write_message(message: dict) -> None:
    raw = json.dumps(message, ensure_ascii=False).encode("utf-8")
    sys.stdout.buffer.write(struct.pack("<I", len(raw)))
    sys.stdout.buffer.write(raw)
    sys.stdout.buffer.flush()


def main() -> int:
    while True:
        message = None
        try:
            message = _read_message()
            if message is None:
                return 0
            response = dispatch(message)
            request_id = str(message.get("__request_id", ""))
            if request_id:
                response = {**response, "__request_id": request_id}
            _write_message(response)
        except Exception as exc:
            request_id = str(message.get("__request_id", "")) if isinstance(message, dict) else ""
            response = {"ok": False, "error": str(exc)}
            if request_id:
                response["__request_id"] = request_id
            _write_message(response)


if __name__ == "__main__":
    raise SystemExit(main())
