from __future__ import annotations

from fnmatch import fnmatch
from urllib.parse import urlsplit

from .config import settings
from .request_context import sanitize_request_headers

_SITE_PROXY_MODES = {"direct", "system", "manual"}


def normalize_site_proxy(raw) -> tuple[str, str]:
    """Return (mode, url). Empty mode means inherit the global proxy."""
    if not isinstance(raw, dict):
        return "", ""
    mode = str(raw.get("proxy_mode") or "").strip().lower()
    if mode not in _SITE_PROXY_MODES:
        mode = ""
    url = str(raw.get("proxy_url") or "").strip()[:2048]
    if mode != "manual":
        url = ""
    return mode, url


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
        try:
            concurrency = max(0, min(64, int(raw.get("concurrency") or 0)))
        except (TypeError, ValueError):
            concurrency = 0
        try:
            speed_limit = max(0, min(1048576, int(raw.get("speed_limit_kib") or 0)))
        except (TypeError, ValueError):
            speed_limit = 0
        proxy_mode, proxy_url = normalize_site_proxy(raw)
        return {
            "host": pattern,
            "user_agent": str(raw.get("user_agent") or "")[:2048],
            "referer": str(raw.get("referer") or "")[:4096],
            "origin": str(raw.get("origin") or "")[:1024],
            "cookie": str(raw.get("cookie") or "")[: 16 * 1024],
            "download_dir": str(raw.get("download_dir") or "").strip()[:32767],
            "request_headers": sanitize_request_headers(raw.get("request_headers")),
            "concurrency": concurrency,
            "speed_limit_kib": speed_limit,
            "proxy_mode": proxy_mode,
            "proxy_url": proxy_url,
        }
    return {}

def site_host_from_url(url: str) -> str:
    try:
        return (urlsplit(str(url or "")).hostname or "").lower().rstrip(".")
    except Exception:
        return ""


def site_profile_from_task(task) -> dict:
    host = site_host_from_url(getattr(task, "url", ""))
    if not host:
        raise ValueError("task has no hostname")
    engine = getattr(task, "engine_state", None) or {}
    try:
        concurrency = max(0, min(64, int(getattr(task, "concurrency", 0) or 0)))
    except (TypeError, ValueError):
        concurrency = 0
    try:
        speed_limit = max(0, min(1048576, int(getattr(task, "speed_limit_kib", 0) or 0)))
    except (TypeError, ValueError):
        speed_limit = 0
    return {
        "host": host[:255],
        "enabled": True,
        "user_agent": str(getattr(task, "user_agent", "") or "")[:2048],
        "referer": str(getattr(task, "referer", "") or "")[:4096],
        "origin": str(getattr(task, "origin", "") or "")[:1024],
        "cookie": str(getattr(task, "cookie", "") or "")[: 16 * 1024],
        "download_dir": str(engine.get("output_dir") or "").strip()[:32767],
        "request_headers": sanitize_request_headers(getattr(task, "request_headers", None)),
        "concurrency": concurrency,
        "speed_limit_kib": speed_limit,
        "proxy_mode": "",
        "proxy_url": "",
    }


def upsert_site_profile(profiles, profile, *, limit=100):
    host = str((profile or {}).get("host") or "").strip().lower().rstrip(".")
    if not host:
        raise ValueError("host required")
    payload = dict(profile)
    payload["host"] = host[:255]
    current = []
    for item in profiles if isinstance(profiles, list) else []:
        if isinstance(item, dict) and str(item.get("host") or "").strip():
            current.append(dict(item))
    for index, item in enumerate(current):
        existing = str(item.get("host") or "").strip().lower().rstrip(".")
        if existing == host:
            current[index] = payload
            return current[:limit], "updated"
    return [payload, *current][:limit], "created"
