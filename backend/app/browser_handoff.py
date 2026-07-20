from __future__ import annotations

import secrets
import time
from dataclasses import asdict, dataclass


@dataclass
class BrowserHandoff:
    id: str
    url: str
    filename: str
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

    def public(self) -> dict:
        value = asdict(self)
        value.pop("cookie", None)
        value.pop("user_agent", None)
        return value


class BrowserHandoffService:
    def __init__(self, ttl: float = 120.0) -> None:
        self.ttl = ttl
        self._items: dict[str, BrowserHandoff] = {}
        self.last_seen = 0.0
        self.version = ""

    def record_ping(self, version: str = "") -> None:
        self.last_seen = time.time()
        self.version = version

    def status(self) -> dict:
        detected = bool(self.last_seen and time.time() - self.last_seen < 90)
        return {"detected": detected, "seen_before": bool(self.last_seen), "version": self.version}

    def create(self, payload: dict) -> BrowserHandoff:
        self.record_ping(str(payload.get("extension_version", "")))
        self.cleanup()
        item = BrowserHandoff(
            id=secrets.token_urlsafe(12),
            url=str(payload.get("url", "")),
            filename=str(payload.get("filename", "")),
            mime_type=str(payload.get("mime_type", "")),
            source_page_url=str(payload.get("source_page_url", "")),
            referer=str(payload.get("referer", "")),
            origin=str(payload.get("origin", "")),
            cookie=str(payload.get("cookie", "")),
            user_agent=str(payload.get("user_agent", "")),
            size=max(0, int(payload.get("size", 0) or 0)),
            status="pending",
            created_at=time.time(),
        )
        self._items[item.id] = item
        return item

    def get(self, handoff_id: str) -> BrowserHandoff | None:
        self.cleanup()
        return self._items.get(handoff_id)

    def pending(self) -> list[dict]:
        self.cleanup()
        return [item.public() for item in self._items.values() if item.status == "pending"]

    def reject(self, handoff_id: str) -> BrowserHandoff | None:
        item = self.get(handoff_id)
        if item and item.status == "pending":
            item.status = "rejected"
        return item

    def cancel(self, handoff_id: str) -> BrowserHandoff | None:
        item = self.get(handoff_id)
        if item and item.status == "pending":
            item.status = "canceled"
        return item

    def cleanup(self) -> None:
        now = time.time()
        for item in self._items.values():
            if item.status == "pending" and now - item.created_at > self.ttl:
                item.status = "expired"
        stale = [key for key, item in self._items.items() if now - item.created_at > self.ttl * 4]
        for key in stale:
            self._items.pop(key, None)


browser_handoffs = BrowserHandoffService()
