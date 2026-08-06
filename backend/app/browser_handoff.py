from __future__ import annotations

import secrets
import re
import threading
import time
from dataclasses import asdict, dataclass
from urllib.parse import urlsplit

from .version import APP_VERSION
from .naming import is_generic_media_name, suggest_manifest_name
from .request_context import request_origin, sanitize_request_contexts, sanitize_request_headers, sanitize_request_replay


# Browser add-ons have an independent release cadence.  Keep this pinned to
# the newest extension build whose Native Messaging contract is compatible;
# desktop-only fixes must not manufacture a browser upgrade prompt.
RECOMMENDED_BROWSER_EXTENSION_VERSION = "3.0.17"
MIN_BROWSER_EXTENSION_VERSION = "2.0.11"
BROWSER_EXTENSION_RELEASE_URL = "https://github.com/ciaooo55/hls-downloader/releases/latest"
DEFAULT_BROWSER_CLIENT_TTL = 180.0
MAX_BROWSER_CLIENTS = 64
MAX_BROWSER_HANDOFFS = 256
BROWSER_CLIENT_HISTORY_TTL = 7 * 24 * 60 * 60.0
_CLIENT_REQUEST_ID_RE = re.compile(r"^[A-Za-z0-9._:-]{1,160}$")


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


@dataclass
class BrowserClient:
    id: str
    browser: str
    version: str
    last_seen: float

    def public(self, now: float, ttl: float) -> dict:
        return {
            "id": self.id,
            "browser": self.browser,
            "version": self.version,
            "last_seen": self.last_seen,
            "active": now - self.last_seen < ttl,
            "needs_upgrade": bool(self.version)
            and _is_older_version(self.version, RECOMMENDED_BROWSER_EXTENSION_VERSION),
        }


def _browser_name(value: str) -> str:
    normalized = str(value or "").strip().lower()
    return normalized if normalized in {
        "edge", "chrome", "chromium", "brave", "vivaldi", "opera", "firefox",
    } else "unknown"


class BrowserHandoffService:
    def __init__(self, ttl: float = 120.0, client_ttl: float = DEFAULT_BROWSER_CLIENT_TTL) -> None:
        self.ttl = ttl
        self.client_ttl = max(30.0, float(client_ttl))
        self._items: dict[str, BrowserHandoff] = {}
        self._clients: dict[str, BrowserClient] = {}
        self._request_ids: dict[str, str] = {}
        self._lock = threading.RLock()

    def record_ping(self, version: str = "", client_id: str = "", browser: str = "") -> None:
        browser_name = _browser_name(browser)
        version = str(version or "").strip()
        client_id = str(client_id or "").strip()[:128]
        if not client_id:
            # Legacy clients have no installation ID. Keep each browser/version
            # in its own slot so an older heartbeat cannot overwrite a newer one.
            client_id = f"legacy:{browser_name}:{version or 'unknown'}"
        with self._lock:
            self._cleanup_clients_locked(time.time())
            self._clients[client_id] = BrowserClient(
                id=client_id,
                browser=browser_name,
                version=version,
                last_seen=time.time(),
            )
            self._trim_clients_locked()

    def status(self) -> dict:
        now = time.time()
        with self._lock:
            self._cleanup_clients_locked(now)
            clients = [client.public(now, self.client_ttl) for client in self._clients.values()]
        clients.sort(key=lambda item: (not item["active"], -float(item["last_seen"]), item["browser"]))
        active_clients = [client for client in clients if client["active"]]
        active_versions = sorted(
            {str(client["version"]) for client in active_clients if client["version"]},
            key=_version_parts,
            reverse=True,
        )
        version = active_versions[0] if active_versions else (str(clients[0]["version"]) if clients else "")
        detected = bool(active_clients)
        seen_before = bool(clients)
        # When at least one extension is connected, the aggregate badge must
        # describe those active clients only. An idle old browser stays in the
        # detailed history, but must not turn a currently connected current
        # client into the contradictory "current version needs itself" warning. With no
        # active client, keep the historical warning for troubleshooting.
        upgrade_scope = active_clients if active_clients else clients
        needs_upgrade = any(bool(client["needs_upgrade"]) for client in upgrade_scope)
        state = "connected" if detected else "inactive" if seen_before else "not_detected"
        message = (
            f"检测到 {len(active_clients)} 个浏览器插件，其中有旧版本，建议升级到 v{RECOMMENDED_BROWSER_EXTENSION_VERSION}"
            if detected and needs_upgrade
            else
            f"已连接 {len(active_clients)} 个浏览器插件"
            if detected
            else f"此前连接的浏览器插件版本较旧，建议升级到 v{RECOMMENDED_BROWSER_EXTENSION_VERSION}"
            if seen_before and needs_upgrade
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
            "release_url": BROWSER_EXTENSION_RELEASE_URL,
            "needs_upgrade": needs_upgrade,
            "clients": clients,
            "active_versions": active_versions,
            "client_count": len(active_clients),
        }

    def create(self, payload: dict) -> BrowserHandoff:
        extension_version = str(payload.get("extension_version", ""))
        if extension_version:
            self.record_ping(
                extension_version,
                str(payload.get("extension_client_id", "")),
                str(payload.get("extension_browser", "")),
            )
        self.cleanup()
        client_request_id = str(payload.get("client_request_id", "") or "").strip()
        client_id = str(payload.get("extension_client_id", "") or "").strip()[:128]
        request_key = (
            f"{client_id or 'legacy'}:{client_request_id}"
            if _CLIENT_REQUEST_ID_RE.fullmatch(client_request_id)
            else ""
        )
        if request_key:
            with self._lock:
                existing_id = self._request_ids.get(request_key, "")
                existing = self._items.get(existing_id) if existing_id else None
                if existing is not None and existing.status in {"pending", "accepting"}:
                    return existing
                self._request_ids.pop(request_key, None)
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
            request_contexts=sanitize_request_contexts(payload.get("request_contexts")),
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
            # Direct HTTP may have created the handoff even if its response was
            # lost and the extension fell back to Native Messaging. Recheck
            # under the lock so that transport retry stays exactly-once.
            if request_key:
                existing_id = self._request_ids.get(request_key, "")
                existing = self._items.get(existing_id) if existing_id else None
                if existing is not None and existing.status in {"pending", "accepting"}:
                    return existing
                self._request_ids.pop(request_key, None)
            self._items[item.id] = item
            if request_key:
                self._request_ids[request_key] = item.id
            while len(self._items) > MAX_BROWSER_HANDOFFS:
                oldest_id = min(self._items, key=lambda key: self._items[key].created_at)
                self._items.pop(oldest_id, None)
                for key, handoff_id in list(self._request_ids.items()):
                    if handoff_id == oldest_id:
                        self._request_ids.pop(key, None)
        return item

    def _cleanup_clients_locked(self, now: float) -> None:
        retention = max(BROWSER_CLIENT_HISTORY_TTL, self.client_ttl * 4)
        for client_id, client in list(self._clients.items()):
            if now - client.last_seen > retention:
                self._clients.pop(client_id, None)

    def _trim_clients_locked(self) -> None:
        if len(self._clients) <= MAX_BROWSER_CLIENTS:
            return
        oldest = sorted(self._clients.values(), key=lambda client: client.last_seen)
        for client in oldest[: len(self._clients) - MAX_BROWSER_CLIENTS]:
            self._clients.pop(client.id, None)

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
            self._cleanup_clients_locked(now)
            self._trim_clients_locked()
            for item in self._items.values():
                if item.status in {"pending", "accepting"} and now - item.created_at > self.ttl:
                    item.status = "expired"
            stale = [key for key, item in self._items.items() if now - item.created_at > self.ttl * 4]
            for key in stale:
                self._items.pop(key, None)
            for key, handoff_id in list(self._request_ids.items()):
                if handoff_id not in self._items:
                    self._request_ids.pop(key, None)


browser_handoffs = BrowserHandoffService()
