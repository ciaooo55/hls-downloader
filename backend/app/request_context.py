from __future__ import annotations

import base64
import binascii
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
# These fields describe a particular browser build or its HTTP stack.  They
# cannot be safely replayed alongside curl-cffi impersonation: a captured
# Firefox/Chrome version, sec-ch-ua brand list and HTTP priority value often
# disagree with the TLS/header fingerprint curl-cffi actually emits.
_BROWSER_FINGERPRINT_HEADERS = {
    "accept",
    "accept-encoding",
    "accept-language",
    "priority",
    "user-agent",
}
_REPLAYABLE_POST_CONTENT_TYPES = {
    "application/json",
    "application/x-www-form-urlencoded",
}
# A browser handoff is sent through Native Messaging and later persisted with
# DPAPI. Keeping it bounded prevents a page from turning the downloader into a
# general-purpose form/file uploader while still covering signed JSON/form
# download endpoints.
MAX_REPLAY_REQUEST_BODY_BYTES = 128 * 1024


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


def source_page_identity(value: str) -> tuple[str, str]:
    """Return the browser page Referer and Origin for a captured task.

    The address-bar page is the access context for media requests.  It must
    not be inferred from the manifest/CDN URL: doing that turns a cross-site
    player request into a same-site request and is a common cause of 403s.
    URL fragments are omitted because browsers never send them in Referer.
    """
    try:
        parsed = urlsplit(str(value or "").strip())
        origin = request_origin(value)
        if not origin:
            return "", ""
        return parsed._replace(fragment="").geturl(), origin
    except (TypeError, ValueError):
        return "", ""


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


def _browser_safe_headers(values: Mapping[str, str] | None) -> dict[str, str]:
    """Drop captured browser-fingerprint fields before a download request.

    Authentication and application-specific headers stay intact.  Referer,
    Origin and Cookie are restored separately by :func:`build_task_headers`,
    so this only removes fields that curl-cffi must own to keep its browser
    impersonation internally consistent.
    """
    headers = sanitize_request_headers(values)
    return {
        name: value
        for name, value in headers.items()
        if name not in _BROWSER_FINGERPRINT_HEADERS and not name.startswith("sec-")
    }


def sanitize_request_replay(
    method: object,
    body: object,
    request_headers: Mapping[str, str] | None,
) -> tuple[str, str]:
    """Return a bounded, safe-to-repeat browser download request.

    Only explicit JSON and URL-encoded POST download requests are replayable.
    File uploads, multipart forms, arbitrary methods and malformed payloads are
    intentionally downgraded to a normal GET instead of being guessed or
    repeated with potentially destructive side effects.
    """
    if str(method or "GET").strip().upper() != "POST" or not isinstance(body, str):
        return "GET", ""
    headers = sanitize_request_headers(request_headers)
    content_type = headers.get("content-type", "").split(";", 1)[0].strip().lower()
    if content_type not in _REPLAYABLE_POST_CONTENT_TYPES:
        return "GET", ""
    # Base64 overhead is 4/3; reject before decoding to bound CPU and memory.
    if not body or len(body) > ((MAX_REPLAY_REQUEST_BODY_BYTES + 2) // 3) * 4:
        return "GET", ""
    try:
        decoded = base64.b64decode(body.encode("ascii"), validate=True)
    except (UnicodeEncodeError, ValueError, binascii.Error):
        return "GET", ""
    if not decoded or len(decoded) > MAX_REPLAY_REQUEST_BODY_BYTES:
        return "GET", ""
    # Canonical encoding keeps database comparisons deterministic and prevents
    # whitespace/newline variants from crossing the JSON/native boundary.
    return "POST", base64.b64encode(decoded).decode("ascii")


def replay_request_body(method: object, body: object, request_headers: Mapping[str, str] | None) -> bytes:
    """Decode a previously validated POST payload, or return an empty body."""
    safe_method, safe_body = sanitize_request_replay(method, body, request_headers)
    if safe_method != "POST":
        return b""
    return base64.b64decode(safe_body)


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
    """Build access headers while leaving browser fingerprinting to the client."""
    page_referer, page_origin = source_page_identity(
        getattr(task, "source_page_url", "")
    )
    target_origin = request_origin(request_url or getattr(task, "url", ""))
    source_origin = request_origin(getattr(task, "url", ""))
    contexts = sanitize_request_contexts(getattr(task, "request_contexts", {}))
    scoped = contexts.get(target_origin) if target_origin else None
    captured_headers = sanitize_request_headers(
        scoped.get("request_headers") if scoped else getattr(task, "request_headers", {})
    )
    captured_access_headers = _browser_safe_headers(captured_headers)
    supplied_headers = _browser_safe_headers(base_headers)
    supplied_values = {
        str(name).lower(): str(value).strip()
        for name, value in dict(base_headers or {}).items()
        if str(value or "").strip() and "\r" not in str(value) and "\n" not in str(value)
    }
    headers = dict(captured_access_headers)
    # Callers may add request-specific access fields (for example an API
    # authorization token). Browser fingerprint fields are intentionally
    # ignored so curl-cffi can inject a coherent profile of its own.
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
        headers.update(captured_access_headers)
    lowered = {name.lower(): name for name in headers}
    scoped_referer = (
        captured_headers.get("referer", "") or str((scoped or {}).get("referer", ""))
        if scoped else ""
    )
    scoped_origin = (
        captured_headers.get("origin", "") or str((scoped or {}).get("origin", ""))
        if scoped else ""
    )
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
    supplied_referer = supplied_values.get("referer", "")
    supplied_origin = supplied_values.get("origin", "")
    supplied_cookie = supplied_values.get("cookie", "")
    set_header(
        "Referer",
        page_referer
        or scoped_referer
        or supplied_referer
        or getattr(task, "referer", "")
        or ("" if browser_context else settings.default_referer),
    )
    set_header(
        "Origin",
        page_origin
        or scoped_origin
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
