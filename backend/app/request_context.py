from __future__ import annotations

import re
from collections.abc import Mapping
from urllib.parse import urlsplit

from .config import settings


_HEADER_NAME = re.compile(r"^[!#$%&'*+.^_`|~0-9A-Za-z-]{1,128}$")
_HOP_BY_HOP = {
    "connection",
    "content-length",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "range",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
}
_CLIENT_MANAGED = {"accept-encoding", "cookie"}


def request_origin(value: str) -> str:
    try:
        parsed = urlsplit(str(value or ""))
        if parsed.scheme not in {"http", "https"} or not parsed.hostname:
            return ""
        host = parsed.hostname.lower()
        if ":" in host and not host.startswith("["):
            host = f"[{host}]"
        default_port = 80 if parsed.scheme == "http" else 443
        port = f":{parsed.port}" if parsed.port and parsed.port != default_port else ""
        return f"{parsed.scheme}://{host}{port}"
    except (TypeError, ValueError):
        return ""


def sanitize_request_headers(values: Mapping[str, str] | None) -> dict[str, str]:
    """Keep replay-safe browser headers and reject transport-owned fields."""
    result: dict[str, str] = {}
    total = 0
    for raw_name, raw_value in list((values or {}).items())[:64]:
        name = str(raw_name or "").strip()
        value = str(raw_value or "").strip()
        lowered = name.lower()
        if (
            not name
            or not value
            or not _HEADER_NAME.fullmatch(name)
            or lowered in _HOP_BY_HOP
            or lowered in _CLIENT_MANAGED
            or "\r" in value
            or "\n" in value
        ):
            continue
        total += len(name) + len(value)
        if total > 32 * 1024:
            break
        # HTTP header names are case-insensitive. Normalizing them also prevents
        # a crafted payload from storing duplicate Authorization/header values
        # under different casing.
        result[lowered] = value
    return result


def sanitize_request_contexts(values: Mapping | None) -> dict[str, dict]:
    """Validate and bound per-origin browser identities before encrypted storage."""
    result: dict[str, dict] = {}
    total = 0
    for raw_origin, raw_context in list((values or {}).items())[:12]:
        origin = request_origin(str(raw_origin or ""))
        if not origin or not isinstance(raw_context, Mapping):
            continue
        context: dict[str, object] = {
            "request_headers": sanitize_request_headers(raw_context.get("request_headers")),
        }
        for key, limit in (
            ("referer", 4096),
            ("origin", 1024),
            ("user_agent", 2048),
            ("cookie", 16 * 1024),
        ):
            value = str(raw_context.get(key, "") or "").strip()
            if "\r" in value or "\n" in value:
                value = ""
            context[key] = value[:limit]
        size = len(origin) + len(str(context))
        if total + size > 96 * 1024:
            break
        total += size
        result[origin] = context
    return result


def build_task_headers(
    task,
    *,
    accept: str = "*/*",
    request_url: str = "",
    base_headers: Mapping[str, str] | None = None,
) -> dict[str, str]:
    """Reproduce a captured browser request without replaying unsafe headers."""
    target_origin = request_origin(request_url or getattr(task, "url", ""))
    source_origin = request_origin(getattr(task, "url", ""))
    contexts = sanitize_request_contexts(getattr(task, "request_contexts", {}))
    scoped = contexts.get(target_origin) if target_origin else None
    captured_headers = sanitize_request_headers(
        scoped.get("request_headers") if scoped else getattr(task, "request_headers", {})
    )
    supplied_headers = sanitize_request_headers(base_headers)
    supplied_values = {
        str(name).lower(): str(value).strip()
        for name, value in dict(base_headers or {}).items()
        if str(value or "").strip() and "\r" not in str(value) and "\n" not in str(value)
    }
    headers = dict(captured_headers)
    # Callers may add request-specific fields (for example Accept or a browser
    # User-Agent). Keep them unless an exact per-origin browser context below
    # has a more authoritative value.
    headers.update(supplied_headers)
    cross_origin_without_context = bool(
        request_url and target_origin and source_origin
        and target_origin != source_origin and scoped is None
    )
    cross_origin = bool(
        request_url and target_origin and source_origin and target_origin != source_origin
    )
    if cross_origin:
        # Never copy origin-bound credentials from the manifest request to a
        # CDN. An exact scoped context may add that CDN's own authorization.
        headers.pop("authorization", None)
        supplied_values.pop("cookie", None)
    if scoped:
        headers.update(captured_headers)
    lowered = {name.lower(): name for name in headers}

    def set_header(name: str, value: str) -> None:
        existing = lowered.get(name.lower())
        if existing and existing != name:
            headers.pop(existing, None)
        if value:
            headers[name] = value
            lowered[name.lower()] = name

    inherit_default_headers = bool(
        dict(getattr(task, "engine_state", {}) or {}).get("inherit_default_headers", True)
    )
    browser_context = not inherit_default_headers or bool(
        getattr(task, "source_page_url", "")
        or getattr(task, "request_headers", {})
        or contexts
    )
    supplied_user_agent = supplied_values.get("user-agent", "")
    supplied_referer = supplied_values.get("referer", "")
    supplied_origin = supplied_values.get("origin", "")
    supplied_cookie = supplied_values.get("cookie", "")
    set_header(
        "User-Agent",
        str((scoped or {}).get("user_agent", ""))
        or supplied_user_agent
        or getattr(task, "user_agent", "")
        or settings.default_user_agent,
    )
    set_header(
        "Referer",
        str((scoped or {}).get("referer", ""))
        or supplied_referer
        or getattr(task, "referer", "")
        or ("" if browser_context else settings.default_referer),
    )
    set_header(
        "Origin",
        str((scoped or {}).get("origin", ""))
        or supplied_origin
        or getattr(task, "origin", "")
        or ("" if browser_context else settings.default_origin),
    )
    set_header(
        "Cookie",
        str((scoped or {}).get("cookie", ""))
        or ("" if cross_origin else supplied_cookie)
        or ("" if cross_origin else getattr(task, "cookie", ""))
        or ("" if browser_context else settings.default_cookie),
    )
    if accept and "accept" not in lowered:
        headers["Accept"] = accept
    return headers
