"""Resident native-shell contract: tray process, pre-created windows, snapshot paint.

The shipping Tauri UI still long-polls `/desktop/session/commands`. This module
is the replacement hot path: a supervisor that is already running, with hidden
confirmation/progress/complete surfaces that can be shown without starting a
WebView. Python remains the download core.
"""

from __future__ import annotations

from collections import deque
from typing import Any
import json
import struct
import threading
import time


PROTOCOL_NAME = "hls-downloader-native-shell"
PROTOCOL_VERSION = 1
MAX_FRAME_BYTES = 1024 * 1024
PAINT_KEYS = ("id", "url", "filename", "title", "mime_type", "size", "resource_kind", "status")
WINDOW_NAMES = ("handoff", "progress", "complete")


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
    return snapshot


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

    def open_main(self) -> None:
        if not self.resident:
            raise RuntimeError("桌面界面尚未就绪")
        self.main_open = True

    def hide_main(self) -> None:
        """Close the task list without quitting. Tray and overlays stay."""
        self.main_open = False

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
        if not self.resident or not self.windows["handoff"]:
            raise RuntimeError("桌面界面尚未就绪")
        self.ensure_core()
        snapshot = paint_snapshot(handoff)
        if not snapshot["id"]:
            raise ValueError("handoff snapshot missing id")
        with self._lock:
            self._sequence += 1
            event = self._event("handoff", snapshot["id"], snapshot)
            event["presentable"] = True
            self._events.append(event)
            self._lock.notify_all()
            return event

    def progress(self, tasks: list[dict[str, Any]]) -> dict[str, Any]:
        if not self.resident or not self.windows["progress"]:
            raise RuntimeError("桌面界面尚未就绪")
        with self._lock:
            self._sequence += 1
            event = self._event("progress")
            event["tasks"] = list(tasks)
            event["presentable"] = True
            self._events.append(event)
            self._lock.notify_all()
            return event

    def complete(self, item: dict[str, Any]) -> dict[str, Any]:
        if not self.resident or not self.windows["complete"]:
            raise RuntimeError("桌面界面尚未就绪")
        with self._lock:
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
