from __future__ import annotations

import secrets
import threading
import time
from dataclasses import asdict, dataclass


@dataclass
class BrowserHandoff:
    id: str
    url: str
    filename: str
    title: str
    mime_type: str
    source_page_url: str
    referer: str
    origin: str
    cookie: str
    user_agent: str
    size: int
    status: str
    created_at: float
    task_id: str = ""
    presented: bool = False
    presentation: str = "pending"
    presentation_error: str = ""

    def public(self) -> dict:
        value = asdict(self)
        value.pop("cookie", None)
        value.pop("user_agent", None)
        return value


class BrowserHandoffService:
    def __init__(self, ttl: float = 120.0) -> None:
        self.ttl = ttl
        self._items: dict[str, BrowserHandoff] = {}
        self._lock = threading.RLock()
        self.last_seen = 0.0
        self.version = ""

    def record_ping(self, version: str = "") -> None:
        with self._lock:
            self.last_seen = time.time()
            self.version = version

    def status(self) -> dict:
        with self._lock:
            last_seen = self.last_seen
            version = self.version
        detected = bool(last_seen and time.time() - last_seen < 90)
        seen_before = bool(last_seen)
        state = "connected" if detected else "inactive" if seen_before else "not_detected"
        message = (
            "浏览器扩展已连接"
            if detected
            else "扩展此前连接过，目前没有心跳"
            if seen_before
            else "未检测到浏览器扩展；浏览器下载不会被接管"
        )
        return {
            "detected": detected,
            "seen_before": seen_before,
            "version": version,
            "state": state,
            "message": message,
        }

    def create(self, payload: dict) -> BrowserHandoff:
        self.record_ping(str(payload.get("extension_version", "")))
        self.cleanup()
        item = BrowserHandoff(
            id=secrets.token_urlsafe(12),
            url=str(payload.get("url", "")),
            filename=str(payload.get("filename", "")),
            title=str(payload.get("title", "")),
            mime_type=str(payload.get("mime_type", "")),
            source_page_url=str(payload.get("source_page_url", "")),
            referer=str(payload.get("referer", "")),
            origin=str(payload.get("origin", "")),
            cookie=str(payload.get("cookie", "")),
            user_agent=str(payload.get("user_agent", "")),
            size=max(0, int(payload.get("size", 0) or 0)),
            status="pending",
            created_at=time.time(),
            presented=False,
            presentation="pending",
        )
        with self._lock:
            self._items[item.id] = item
        return item

    def get(self, handoff_id: str) -> BrowserHandoff | None:
        self.cleanup()
        with self._lock:
            return self._items.get(handoff_id)

    def pending(self) -> list[dict]:
        self.cleanup()
        with self._lock:
            return [item.public() for item in self._items.values() if item.status == "pending"]

    def mark_presentation(self, handoff_id: str, presentation: str, error: str = "") -> BrowserHandoff | None:
        presentation = str(presentation or "").strip().lower()
        if presentation not in {"pending", "queued", "presented", "failed"}:
            raise ValueError(f"unsupported presentation state: {presentation}")
        rank = {"pending": 0, "queued": 1, "presented": 2, "failed": 2}
        with self._lock:
            item = self._items.get(handoff_id)
            if not item:
                return None
            # Never downgrade a successful presentation back to queued/pending.
            if rank[presentation] < rank.get(item.presentation, 0) and item.presentation != "failed":
                return item
            item.presentation = presentation
            item.presented = presentation == "presented"
            item.presentation_error = str(error or "") if presentation == "failed" else ""
            return item

    def claim(self, handoff_id: str) -> BrowserHandoff | None:
        """Atomically claim a pending handoff so only one accept path can create a task."""
        with self._lock:
            item = self._items.get(handoff_id)
            if not item or item.status != "pending":
                return None
            if time.time() - item.created_at > self.ttl:
                item.status = "expired"
                return None
            item.status = "accepting"
            return item

    def complete_accept(self, handoff_id: str, task_id: str) -> BrowserHandoff | None:
        with self._lock:
            item = self._items.get(handoff_id)
            if not item:
                return None
            item.status = "accepted"
            item.task_id = task_id
            return item

    def fail_accept(self, handoff_id: str) -> BrowserHandoff | None:
        with self._lock:
            item = self._items.get(handoff_id)
            if not item:
                return None
            if item.status == "accepting":
                item.status = "pending"
            return item

    def reject(self, handoff_id: str) -> BrowserHandoff | None:
        with self._lock:
            item = self._items.get(handoff_id)
            if item and item.status == "pending":
                item.status = "rejected"
            return item

    def cancel(self, handoff_id: str) -> BrowserHandoff | None:
        with self._lock:
            item = self._items.get(handoff_id)
            if item and item.status == "pending":
                item.status = "canceled"
            return item

    def cleanup(self) -> None:
        now = time.time()
        with self._lock:
            for item in self._items.values():
                if item.status in {"pending", "accepting"} and now - item.created_at > self.ttl:
                    item.status = "expired"
            stale = [key for key, item in self._items.items() if now - item.created_at > self.ttl * 4]
            for key in stale:
                self._items.pop(key, None)


browser_handoffs = BrowserHandoffService()
