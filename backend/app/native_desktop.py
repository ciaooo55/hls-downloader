from __future__ import annotations

from collections import deque
from dataclasses import dataclass
import threading
import time
from typing import Callable


@dataclass(frozen=True)
class DesktopCommand:
    sequence: int
    kind: str
    handoff_id: str = ""

    def public(self) -> dict:
        return {
            "sequence": self.sequence,
            "kind": self.kind,
            "handoff_id": self.handoff_id,
        }


class NativeDesktopSession:
    """Thread-safe command bridge for the out-of-process desktop UI."""

    def __init__(self, history_size: int = 128) -> None:
        self._lock = threading.RLock()
        self._changed = threading.Condition(self._lock)
        self._active = False
        self._started_at = 0.0
        self._sequence = 0
        self._commands: deque[DesktopCommand] = deque(maxlen=history_size)

    def start(self) -> dict:
        with self._changed:
            self._active = True
            self._started_at = time.time()
            self._changed.notify_all()
            return self.status()

    def stop(self) -> dict:
        with self._changed:
            self._active = False
            self._changed.notify_all()
            return self.status()

    def status(self) -> dict:
        with self._lock:
            return {
                "ok": True,
                "active": self._active,
                "sequence": self._sequence,
                "started_at": self._started_at,
            }

    def push(self, kind: str, handoff_id: str = "") -> bool:
        with self._changed:
            if not self._active:
                return False
            self._sequence += 1
            self._commands.append(DesktopCommand(self._sequence, kind, handoff_id))
            self._changed.notify_all()
            return True

    def activate(self) -> bool:
        return self.push("activate")

    def shutdown(self) -> bool:
        return self.push("shutdown")

    def handoff(self, handoff_id: str) -> None:
        if not self.push("handoff", str(handoff_id)):
            raise RuntimeError("桌面界面尚未就绪")

    def poll(self, after: int = 0, timeout: float = 20.0) -> dict:
        timeout = max(0.0, min(float(timeout), 25.0))
        deadline = time.monotonic() + timeout
        with self._changed:
            while self._active and self._sequence <= after:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                self._changed.wait(remaining)
            commands = [item.public() for item in self._commands if item.sequence > after]
            return {
                "ok": True,
                "active": self._active,
                "sequence": self._sequence,
                "commands": commands,
            }


native_desktop_session = NativeDesktopSession()

_core_shutdown: Callable[[], None] | None = None
_core_shutdown_lock = threading.Lock()


def register_core_shutdown(callback: Callable[[], None] | None) -> None:
    global _core_shutdown
    with _core_shutdown_lock:
        _core_shutdown = callback


def request_core_shutdown() -> bool:
    with _core_shutdown_lock:
        callback = _core_shutdown
    if callback is None:
        return False
    callback()
    return True
