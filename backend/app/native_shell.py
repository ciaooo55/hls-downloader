"""Resident native-shell contract: tray process, pre-created windows, snapshot paint.

The shipping Tauri UI still long-polls `/desktop/session/commands`. This module
is the replacement hot path: a supervisor that is already running, with hidden
confirmation/progress/complete surfaces that can be shown without starting a
WebView. Python remains the download core.
"""

from __future__ import annotations

from collections import deque
from pathlib import Path
from typing import Any
import json
import os
import socket
import struct
import subprocess
import threading
import time


PROTOCOL_NAME = "hls-downloader-native-shell"
PROTOCOL_VERSION = 1
MAX_FRAME_BYTES = 1024 * 1024
PAINT_KEYS = ("id", "url", "filename", "title", "mime_type", "size", "resource_kind", "status")
WINDOW_NAMES = ("handoff", "progress", "complete")
IPC_OPS = (
    "hello",
    "boot",
    "status",
    "offer",
    "progress",
    "complete",
    "wait",
    "open_main",
    "hide_main",
    "shutdown",
)

_supervisor_lock = threading.RLock()
_supervisor: NativeShellSupervisor | None = None
_ipc_server: NativeShellIpcServer | None = None


def paint_snapshot(handoff: dict[str, Any] | None) -> dict[str, Any]:
    """Fields the confirmation window can draw before any extra HTTP round-trip."""
    source = handoff if isinstance(handoff, dict) else {}
    snapshot = {key: source.get(key) for key in PAINT_KEYS}
    snapshot["id"] = str(snapshot.get("id") or "")
    snapshot["url"] = str(snapshot.get("url") or "")
    snapshot["filename"] = str(snapshot.get("filename") or "")
    snapshot["title"] = str(snapshot.get("title") or "")
    snapshot["mime_type"] = str(snapshot.get("mime_type") or "")
    snapshot["resource_kind"] = str(snapshot.get("resource_kind") or "file")
    snapshot["status"] = str(snapshot.get("status") or "pending")
    try:
        snapshot["size"] = max(0, int(snapshot.get("size") or 0))
    except (TypeError, ValueError):
        snapshot["size"] = 0
    # Save folder is always the local setting, never a field from the offer.
    snapshot["download_dir"] = local_download_dir()
    return snapshot


def local_download_dir() -> str:
    try:
        from .config import settings

        return str(getattr(settings, "download_dir", "") or "")
    except Exception:
        return ""


def encode_frame(message: dict[str, Any]) -> bytes:
    payload = json.dumps(message, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(payload) > MAX_FRAME_BYTES:
        raise ValueError("native shell frame too large")
    return struct.pack("<I", len(payload)) + payload


def decode_frame(buffer: bytes) -> dict[str, Any]:
    if len(buffer) < 4:
        raise ValueError("native shell frame truncated")
    (length,) = struct.unpack("<I", buffer[:4])
    if length > MAX_FRAME_BYTES:
        raise ValueError("native shell frame too large")
    if len(buffer) < 4 + length:
        raise ValueError("native shell frame truncated")
    message = json.loads(buffer[4:4 + length].decode("utf-8"))
    if not isinstance(message, dict):
        raise ValueError("native shell frame is not an object")
    return message


def _recv_exact(sock: socket.socket, size: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < size:
        piece = sock.recv(size - len(chunks))
        if not piece:
            raise ConnectionError("native shell connection closed")
        chunks.extend(piece)
    return bytes(chunks)


def read_frame(sock: socket.socket) -> dict[str, Any]:
    header = _recv_exact(sock, 4)
    (length,) = struct.unpack("<I", header)
    if length > MAX_FRAME_BYTES:
        raise ValueError("native shell frame too large")
    payload = _recv_exact(sock, length)
    return decode_frame(header + payload)


def write_frame(sock: socket.socket, message: dict[str, Any]) -> None:
    sock.sendall(encode_frame(message))


def env_wants_native_shell() -> bool:
    return os.environ.get("HLS_NATIVE_SHELL", "").strip().lower() in {"1", "true", "yes", "on"}


class NativeShellSupervisor:
    """Always-on tray supervisor. Main window is optional; overlays are warm."""

    def __init__(self) -> None:
        self._lock = threading.Condition()
        self.resident = False
        self.core_running = False
        self.main_open = False
        self.windows = {name: False for name in WINDOW_NAMES}
        self._started_at = 0.0
        self._sequence = 0
        self._events: deque[dict[str, Any]] = deque(maxlen=128)

    def boot_resident(self) -> dict[str, Any]:
        """Login/start: tray + hidden dialogs. Do not open the task list."""
        with self._lock:
            self.resident = True
            self.main_open = False
            self.windows = {name: True for name in WINDOW_NAMES}
            self._started_at = time.time()
            self._lock.notify_all()
            return self.status()

    def ensure_core(self) -> None:
        with self._lock:
            self.core_running = True

    def open_main(self) -> dict[str, Any]:
        with self._lock:
            if not self.resident:
                raise RuntimeError("桌面界面尚未就绪")
            self.main_open = True
            return self.status()

    def hide_main(self) -> dict[str, Any]:
        """Close the task list without quitting. Tray and overlays stay."""
        with self._lock:
            self.main_open = False
            return self.status()

    def shutdown(self) -> dict[str, Any]:
        with self._lock:
            self.resident = False
            self.core_running = False
            self.main_open = False
            self.windows = {name: False for name in WINDOW_NAMES}
            self._sequence += 1
            event = self._event("shutdown")
            self._events.append(event)
            self._lock.notify_all()
            return event

    def offer(self, handoff: dict[str, Any]) -> dict[str, Any]:
        """Show confirmation from the offer snapshot. No extra fetch required."""
        with self._lock:
            if not self.resident or not self.windows["handoff"]:
                raise RuntimeError("桌面界面尚未就绪")
            self.core_running = True
            snapshot = paint_snapshot(handoff)
            if not snapshot["id"]:
                raise ValueError("handoff snapshot missing id")
            self._sequence += 1
            event = self._event("handoff", snapshot["id"], snapshot)
            event["presentable"] = True
            self._events.append(event)
            self._lock.notify_all()
            return event

    def progress(self, tasks: list[dict[str, Any]]) -> dict[str, Any]:
        with self._lock:
            if not self.resident or not self.windows["progress"]:
                raise RuntimeError("桌面界面尚未就绪")
            self._sequence += 1
            event = self._event("progress")
            event["tasks"] = list(tasks)
            event["presentable"] = True
            if self._events and self._events[-1].get("kind") == "progress":
                self._events[-1] = event
            else:
                self._events.append(event)
            self._lock.notify_all()
            return event

    def complete(self, item: dict[str, Any]) -> dict[str, Any]:
        with self._lock:
            if not self.resident or not self.windows["complete"]:
                raise RuntimeError("桌面界面尚未就绪")
            self._sequence += 1
            event = self._event("complete")
            event["item"] = dict(item)
            event["presentable"] = True
            self._events.append(event)
            self._lock.notify_all()
            return event

    def wait_event(self, after: int = 0, timeout: float = 1.0) -> dict[str, Any]:
        timeout = max(0.0, min(float(timeout), 5.0))
        deadline = time.monotonic() + timeout
        with self._lock:
            while self.resident and self._sequence <= after:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                self._lock.wait(remaining)
            events = [item for item in self._events if int(item.get("sequence") or 0) > after]
            return {
                "ok": True,
                "resident": self.resident,
                "sequence": self._sequence,
                "events": events,
            }

    def is_ready(self) -> bool:
        with self._lock:
            return bool(self.resident and self.windows.get("handoff"))

    def status(self) -> dict[str, Any]:
        with self._lock:
            return {
                "ok": True,
                "protocol": PROTOCOL_NAME,
                "version": PROTOCOL_VERSION,
                "resident": self.resident,
                "core_running": self.core_running,
                "main_open": self.main_open,
                "windows": dict(self.windows),
                "sequence": self._sequence,
                "started_at": self._started_at,
                "hot_path": "pipe-to-precreated-window",
            }

    def _event(self, kind: str, handoff_id: str = "", snapshot: dict[str, Any] | None = None) -> dict[str, Any]:
        message = {
            "protocol": PROTOCOL_NAME,
            "version": PROTOCOL_VERSION,
            "sequence": self._sequence,
            "kind": kind,
            "handoff_id": handoff_id,
        }
        if snapshot is not None:
            message["snapshot"] = snapshot
        return message


class NativeShellIpcServer:
    """Loopback length-prefixed JSON server. Named-pipe twin for tests and a future Rust attach."""

    def __init__(self, supervisor: NativeShellSupervisor) -> None:
        self.supervisor = supervisor
        self.host = "127.0.0.1"
        self.port = 0
        self._stop = threading.Event()
        self._sock: socket.socket | None = None
        self._thread: threading.Thread | None = None

    def start(self, port: int = 0) -> dict[str, Any]:
        if self._thread is not None and self._thread.is_alive():
            return self.endpoint()
        self._stop.clear()
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", port))
        listener.listen(8)
        listener.settimeout(0.2)
        self._sock = listener
        self.host, self.port = listener.getsockname()[:2]
        self._thread = threading.Thread(target=self._serve, name="native-shell-ipc", daemon=True)
        self._thread.start()
        return self.endpoint()

    def stop(self) -> None:
        self._stop.set()
        sock = self._sock
        self._sock = None
        if sock is not None:
            try:
                sock.close()
            except OSError:
                pass
        thread = self._thread
        if thread is not None:
            thread.join(1.0)
        self._thread = None
        self.port = 0

    def endpoint(self) -> dict[str, Any]:
        return {
            "ok": True,
            "protocol": PROTOCOL_NAME,
            "version": PROTOCOL_VERSION,
            "host": self.host,
            "port": self.port,
            "transport": "tcp-loopback",
        }

    def _serve(self) -> None:
        listener = self._sock
        while listener is not None and not self._stop.is_set():
            try:
                client, _addr = listener.accept()
            except TimeoutError:
                continue
            except OSError:
                break
            threading.Thread(
                target=self._handle_client,
                args=(client,),
                name="native-shell-ipc-client",
                daemon=True,
            ).start()
            listener = self._sock

    def _handle_client(self, client: socket.socket) -> None:
        client.settimeout(8.0)
        try:
            while not self._stop.is_set():
                try:
                    message = read_frame(client)
                except (ConnectionError, TimeoutError, ValueError, OSError):
                    break
                try:
                    reply = dispatch_ipc(self.supervisor, message)
                except (RuntimeError, ValueError, TypeError) as exc:
                    reply = {"ok": False, "error": str(exc)}
                try:
                    write_frame(client, reply)
                except OSError:
                    break
        finally:
            try:
                client.close()
            except OSError:
                pass


def dispatch_ipc(supervisor: NativeShellSupervisor, message: dict[str, Any]) -> dict[str, Any]:
    """One request/response on the length-prefixed JSON pipe."""
    if not isinstance(message, dict):
        raise ValueError("native shell frame is not an object")
    op = str(message.get("op") or message.get("kind") or "").strip().lower()
    if op not in IPC_OPS:
        raise ValueError("不支持的 native shell 操作")
    if op == "hello":
        return {
            "ok": True,
            "protocol": PROTOCOL_NAME,
            "version": PROTOCOL_VERSION,
            "ops": list(IPC_OPS),
        }
    if op == "boot":
        return supervisor.boot_resident()
    if op == "status":
        return supervisor.status()
    if op == "offer":
        handoff = message.get("handoff") or message.get("snapshot") or message
        if not isinstance(handoff, dict):
            raise ValueError("handoff snapshot missing")
        return supervisor.offer(handoff)
    if op == "progress":
        tasks = message.get("tasks")
        if not isinstance(tasks, list):
            raise ValueError("progress tasks missing")
        return supervisor.progress(tasks)
    if op == "complete":
        item = message.get("item")
        if not isinstance(item, dict):
            raise ValueError("complete item missing")
        return supervisor.complete(item)
    if op == "wait":
        return supervisor.wait_event(int(message.get("after") or 0), float(message.get("timeout") or 1.0))
    if op == "open_main":
        return supervisor.open_main()
    if op == "hide_main":
        return supervisor.hide_main()
    return supervisor.shutdown()


def native_shell_supervisor() -> NativeShellSupervisor:
    global _supervisor
    with _supervisor_lock:
        if _supervisor is None:
            _supervisor = NativeShellSupervisor()
        return _supervisor


def is_native_shell_ready() -> bool:
    return native_shell_supervisor().is_ready()


def boot_native_shell() -> dict[str, Any]:
    return native_shell_supervisor().boot_resident()


def shutdown_native_shell() -> dict[str, Any]:
    return native_shell_supervisor().shutdown()


def native_shell_status() -> dict[str, Any]:
    return native_shell_supervisor().status()


def start_native_shell_ipc(port: int = 0) -> dict[str, Any]:
    global _ipc_server
    with _supervisor_lock:
        if _ipc_server is None:
            _ipc_server = NativeShellIpcServer(native_shell_supervisor())
        return _ipc_server.start(port)


def stop_native_shell_ipc() -> None:
    global _ipc_server
    with _supervisor_lock:
        server = _ipc_server
        _ipc_server = None
    if server is not None:
        server.stop()


def reset_native_shell() -> None:
    """Tests and core shutdown: drop resident state without leaking IPC threads."""
    global _supervisor
    stop_native_shell_ipc()
    with _supervisor_lock:
        previous = _supervisor
        _supervisor = NativeShellSupervisor()
    if previous is not None and previous.resident:
        previous.shutdown()


NATIVE_PROGRESS_STATUSES = {
    "fetching_metadata",
    "checking",
    "downloading",
    "downloading_m3u8",
    "parsing",
    "downloading_segments",
    "pausing",
    "merging",
    "remuxing",
}


def overlay_progress_item(task: dict[str, Any] | None) -> dict[str, Any]:
    source = task if isinstance(task, dict) else {}
    try:
        percent = max(0.0, min(100.0, float(source.get("progress_percent") or source.get("percent") or 0)))
    except (TypeError, ValueError):
        percent = 0.0
    try:
        downloaded = max(0, int(source.get("downloaded_bytes") or 0))
    except (TypeError, ValueError):
        downloaded = 0
    try:
        total = max(0, int(source.get("total_bytes") or 0))
    except (TypeError, ValueError):
        total = 0
    try:
        speed = max(0.0, float(source.get("speed_bytes_per_sec") or 0))
    except (TypeError, ValueError):
        speed = 0.0
    try:
        eta = max(0.0, float(source.get("eta_seconds") or 0))
    except (TypeError, ValueError):
        eta = 0.0
    return {
        "id": str(source.get("id") or source.get("task_id") or ""),
        "filename": str(source.get("filename") or source.get("title") or ""),
        "status": str(source.get("status") or ""),
        "progress_percent": percent,
        "downloaded_bytes": downloaded,
        "total_bytes": total,
        "speed_bytes_per_sec": speed,
        "eta_seconds": eta,
        "is_live": bool(source.get("is_live")),
    }


def overlay_complete_item(task: dict[str, Any] | None) -> dict[str, Any]:
    source = task if isinstance(task, dict) else {}
    try:
        downloaded = max(0, int(source.get("downloaded_bytes") or source.get("total_bytes") or 0))
    except (TypeError, ValueError):
        downloaded = 0
    return {
        "id": str(source.get("id") or source.get("task_id") or ""),
        "filename": str(source.get("filename") or source.get("title") or ""),
        "title": str(source.get("title") or source.get("filename") or ""),
        "output_path": str(source.get("output_path") or ""),
        "downloaded_bytes": downloaded,
        "output_is_file": source.get("output_is_file") is not False,
    }


def sync_native_shell_from_event(
    event: dict[str, Any] | None,
    running_tasks: list[dict[str, Any]] | None = None,
) -> None:
    """Drive pre-created progress/complete windows from core task events."""
    if not is_native_shell_ready():
        return
    try:
        from .config import settings
    except Exception:
        return
    supervisor = native_shell_supervisor()
    payload = event if isinstance(event, dict) else {}
    status = str(payload.get("status") or "")
    if status == "done" and getattr(settings, "download_complete_popup_enabled", True) is not False:
        item = overlay_complete_item(payload)
        if item["id"]:
            try:
                supervisor.complete(item)
            except RuntimeError:
                pass
    items: list[dict[str, Any]] = []
    if getattr(settings, "download_progress_window_enabled", True) is not False:
        for task in running_tasks or []:
            item = overlay_progress_item(task)
            if item["id"] and str(item.get("status") or "") in NATIVE_PROGRESS_STATUSES:
                items.append(item)
            if len(items) >= 4:
                break
    try:
        supervisor.progress(items)
    except RuntimeError:
        pass


def running_on_windows() -> bool:
    return os.name == "nt"


def locate_native_shell_executable(project_root: Path | None = None) -> Path | None:
    """Packaged supervisor next to the app image. Source debug builds are ignored."""
    root = Path(project_root or os.environ.get("HLS_NATIVE_SHELL_ROOT") or ".")
    names = ("HLSNativeShell.exe", "hls-native-shell.exe", "hls-native-shell")
    for name in names:
        candidate = root / name
        if candidate.is_file():
            return candidate
    return None


def maybe_spawn_native_shell_process(
    *,
    core_url: str,
    token: str,
    project_root: Path | None = None,
) -> Path | None:
    """Start the resident HWND supervisor on Windows when the packaged binary exists.

    Tests and Linux source runs keep the Tauri/web fallback. The supervisor
    process POSTs /desktop/native-shell/boot after its windows are warm.
    """
    if not running_on_windows():
        return None
    if os.environ.get("PYTEST_CURRENT_TEST"):
        return None
    if os.environ.get("HLS_NATIVE_SHELL", "").strip().lower() in {"0", "false", "off"}:
        return None
    executable = locate_native_shell_executable(project_root)
    if executable is None or not token:
        return None
    # DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP. Do not use CREATE_NO_WINDOW;
    # that hides the pre-created confirmation HWNDs.
    creationflags = 0x00000008 | 0x00000200
    subprocess.Popen(
        [
            str(executable),
            "--core-url",
            core_url,
            "--token",
            token,
        ],
        cwd=str(executable.parent),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        close_fds=False,
        creationflags=creationflags,
    )
    return executable
