from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from urllib.parse import urlparse


@dataclass
class UserscriptSnapshot:
    detected: bool
    seen_before: bool
    version: str
    page_origin: str
    last_seen_at: str


class UserscriptMonitor:
    def __init__(self, freshness_seconds: int = 150) -> None:
        self._freshness = timedelta(seconds=freshness_seconds)
        self._last_seen: datetime | None = None
        self._version = ""
        self._page_origin = ""

    def record(self, version: str = "", page_url: str = "") -> None:
        self._last_seen = datetime.now(timezone.utc)
        self._version = version[:64]
        self._page_origin = self._origin(page_url)

    def snapshot(self) -> UserscriptSnapshot:
        now = datetime.now(timezone.utc)
        detected = self._last_seen is not None and now - self._last_seen <= self._freshness
        return UserscriptSnapshot(
            detected=detected,
            seen_before=self._last_seen is not None,
            version=self._version,
            page_origin=self._page_origin,
            last_seen_at=self._last_seen.isoformat() if self._last_seen else "",
        )

    def reset(self) -> None:
        self._last_seen = None
        self._version = ""
        self._page_origin = ""

    @staticmethod
    def _origin(page_url: str) -> str:
        parsed = urlparse(page_url)
        if parsed.scheme not in {"http", "https"} or not parsed.netloc:
            return ""
        return f"{parsed.scheme}://{parsed.netloc}"


userscript_monitor = UserscriptMonitor()
