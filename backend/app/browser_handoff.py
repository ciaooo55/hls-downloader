from __future__ import annotations

import secrets
import threading
import time
from dataclasses import asdict, dataclass
from urllib.parse import urlsplit

from .version import APP_VERSION
from .naming import is_generic_media_name, suggest_manifest_name
from .request_context import request_origin, sanitize_request_headers, sanitize_request_replay


RECOMMENDED_BROWSER_EXTENSION_VERSION = "3.0.0"
MIN_BROWSER_EXTENSION_VERSION = "2.0.11"


def _version_parts(value: str) -> tuple[int, ...]:
    parts: list[int] = []
    for part in str(value or "").split("."):
        if not part.isdigit():
            break
        parts.append(int(part))
    return tuple(parts)


def _is_older_version(value: str, baseline: str) -> bool:
    current = _version_parts(value)
    target = _version_parts(baseline)
    if not current or not target:
        return False
    size = max(len(current), len(target))
    return current + (0,) * (size - len(current)) < target + (0,) * (size - len(target))


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
    request_headers: dict[str, str]
    request_contexts: dict[str, dict]
    request_method: str
    request_body: str
    size: int
    status: str
    created_at: float
    task_id: str = ""
    presented: bool = False
    presentation: str = "pending"
    presentation_error: str = ""
    resource_kind: str = "file"
    suppression: dict[str, str] | None = None

    def public(self) -> dict:
        value = asdict(self)
        value.pop("cookie", None)
        value.pop("user_agent", None)
        value.pop("request_headers", None)
        value.pop("request_contexts", None)
        value.pop("request_body", None)
        if value.get("suppression") is None:
            value.pop("suppression", None)
        return value

    def effective_context(self) -> dict:
        """Return the exact-origin context shown in the local confirmation window."""
        target_origin = request_origin(self.url)
        scoped = self.request_contexts.get(target_origin, {}) if target_origin else {}
        headers = sanitize_request_headers(
            scoped.get("request_headers") if isinstance(scoped, dict) else self.request_headers
        )
        referer = str((scoped or {}).get("referer") or self.referer or "")
        origin = str((scoped or {}).get("origin") or self.origin or "")
        user_agent = str((scoped or {}).get("user_agent") or self.user_agent or "")
        cookie = str((scoped or {}).get("cookie") or self.cookie or "")
        if referer and "referer" not in headers:
            headers["referer"] = referer
        if origin and "origin" not in headers:
            headers["origin"] = origin
        if user_agent and "user-agent" not in headers:
            headers["user-agent"] = user_agent
        return {
            "target_origin": target_origin,
            "referer": referer,
            "origin": origin,
            "user_agent": user_agent,
            "cookie": cookie,
            "request_headers": headers,
        }

    def detail(self) -> dict:
        """Return public handoff metadata plus the local user's actual context."""
        value = self.public()
        value["effective_context"] = self.effective_context()
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
        needs_upgrade = bool(version) and _is_older_version(version, RECOMMENDED_BROWSER_EXTENSION_VERSION)
        state = "connected" if detected else "inactive" if seen_before else "not_detected"
        message = (
            f"浏览器插件版本低于推荐版本，建议升级到 v{RECOMMENDED_BROWSER_EXTENSION_VERSION}"
            if detected and needs_upgrade
            else
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
            "desktop_version": APP_VERSION,
            "recommended_version": RECOMMENDED_BROWSER_EXTENSION_VERSION,
            "minimum_version": MIN_BROWSER_EXTENSION_VERSION,
            "needs_upgrade": needs_upgrade,
        }

    def create(self, payload: dict) -> BrowserHandoff:
        self.record_ping(str(payload.get("extension_version", "")))
        self.cleanup()
        url = str(payload.get("url", ""))
        filename = str(payload.get("filename", ""))
        title = str(payload.get("title", ""))
        mime_type = str(payload.get("mime_type", ""))
        source_page_url = str(payload.get("source_page_url", ""))
        manifest = ".m3u8" in url.lower() or url.lower().split("?", 1)[0].endswith(".mpd") or any(
            marker in mime_type.lower() for marker in ("mpegurl", "dash+xml")
        )
        if manifest and is_generic_media_name(filename):
            filename = suggest_manifest_name(
                url,
                filename=filename,
                title=title,
                source_page_url=source_page_url,
                fallback="download",
            )
        request_headers = sanitize_request_headers(payload.get("request_headers"))
        request_method, request_body = sanitize_request_replay(
            payload.get("request_method", "GET"), payload.get("request_body", ""), request_headers
        )
        resource_kind = str(payload.get("resource_kind", "file") or "file").lower()
        if resource_kind not in {"hls", "dash", "media", "file", "magnet"}:
            resource_kind = "file"
        item = BrowserHandoff(
            id=secrets.token_urlsafe(12),
            url=url,
            filename=filename,
            title=title,
            mime_type=mime_type,
            source_page_url=source_page_url,
            referer=str(payload.get("referer", "")),
            origin=str(payload.get("origin", "")),
            cookie=str(payload.get("cookie", "")),
            user_agent=str(payload.get("user_agent", "")),
            request_headers=request_headers,
            request_contexts={str(key): dict(value) for key, value in dict(payload.get("request_contexts") or {}).items() if isinstance(value, dict)},
            request_method=request_method,
            request_body=request_body,
            size=max(0, int(payload.get("size", 0) or 0)),
            status="pending",
            created_at=time.time(),
            presented=False,
            presentation="pending",
            resource_kind=resource_kind,
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

    def cancel(
        self,
        handoff_id: str,
        *,
        suppress_site_kind: bool = False,
    ) -> BrowserHandoff | None:
        with self._lock:
            item = self._items.get(handoff_id)
            if item and item.status == "pending":
                item.status = "canceled"
                if suppress_site_kind:
                    host = (urlsplit(item.source_page_url).hostname or "").lower()
                    if host:
                        item.suppression = {
                            "host": host,
                            "kind": item.resource_kind,
                        }
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
