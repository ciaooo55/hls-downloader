from __future__ import annotations

from fnmatch import fnmatch
from urllib.parse import urlsplit

from .config import settings
from .request_context import sanitize_request_headers


def resolve_site_profile(url: str) -> dict:
    """Return the first matching per-host download rule."""
    try:
        host = (urlsplit(str(url or "")).hostname or "").lower().rstrip(".")
    except Exception:
        return {}
    if not host:
        return {}
    for raw in getattr(settings, "site_profiles", []) or []:
        if not isinstance(raw, dict) or raw.get("enabled", True) is False:
            continue
        pattern = str(raw.get("host") or raw.get("pattern") or "").strip().lower().rstrip(".")
        if not pattern or not fnmatch(host, pattern):
            continue
        return {
            "host": pattern,
            "user_agent": str(raw.get("user_agent") or "")[:2048],
            "referer": str(raw.get("referer") or "")[:4096],
            "origin": str(raw.get("origin") or "")[:1024],
            "request_headers": sanitize_request_headers(raw.get("request_headers")),
            "concurrency": max(0, min(64, int(raw.get("concurrency") or 0))),
            "speed_limit_kib": max(0, min(1048576, int(raw.get("speed_limit_kib") or 0))),
        }
    return {}
