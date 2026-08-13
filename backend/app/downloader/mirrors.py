"""Identity-safe HTTP mirror helpers.

Mirrors are extra HTTP(S) locations for the same ordinary file. They must not
change the default single-URL path: an empty list is a no-op. Compatible
mirrors can fail over when the primary dies, or feed extra Range workers when
they advertise the same object length.
"""

from __future__ import annotations

from urllib.parse import urlparse, urlsplit, urlunsplit

MAX_MIRRORS = 16
MAX_MIRROR_URL_LENGTH = 8192


def _strong_etag(value: str) -> str:
    etag = str(value or "").strip()
    return "" if etag.lower().startswith("w/") else etag


def canonical_http_url(value: str) -> str:
    raw = str(value or "").strip()
    if not raw:
        return ""
    try:
        parsed = urlsplit(raw)
    except ValueError:
        return ""
    if parsed.scheme.lower() not in {"http", "https"} or not parsed.hostname:
        return ""
    hostname = parsed.hostname.lower().rstrip(".")
    try:
        port = parsed.port
    except ValueError:
        return ""
    default_port = 443 if parsed.scheme.lower() == "https" else 80
    netloc = hostname
    if port and port != default_port:
        netloc = f"{hostname}:{port}"
    path = parsed.path or "/"
    return urlunsplit((parsed.scheme.lower(), netloc, path, parsed.query, ""))


def normalize_mirror_urls(primary: str, mirrors) -> list[str]:
    """Return unique HTTP(S) mirrors, never including the primary URL."""
    primary_key = canonical_http_url(primary)
    result: list[str] = []
    seen: set[str] = {primary_key} if primary_key else set()
    if mirrors is None:
        return result
    if isinstance(mirrors, str):
        candidates = mirrors.splitlines()
    else:
        try:
            candidates = list(mirrors)
        except TypeError:
            return result
    for raw in candidates:
        url = str(raw or "").strip()
        if not url or url.startswith("#") or len(url) > MAX_MIRROR_URL_LENGTH:
            continue
        parsed = urlparse(url)
        if parsed.scheme.lower() not in {"http", "https"} or not parsed.hostname:
            continue
        key = canonical_http_url(url)
        if not key or key in seen:
            continue
        seen.add(key)
        result.append(url)
        if len(result) >= MAX_MIRRORS:
            break
    return result


def mirror_identity_compatible(
    primary: dict,
    candidate: dict,
    *,
    has_checksum: bool = False,
) -> tuple[bool, str]:
    """Decide whether a probed mirror can share bytes with the primary object.

    Size is mandatory. Matching strong ETag / Last-Modified is preferred.
    Same-size ranged mirrors without validators are accepted only because the
    transfer still checks Content-Range totals and an optional user checksum.
    Different lengths never mix.
    """
    try:
        primary_total = int(primary.get("total") or 0)
        candidate_total = int(candidate.get("total") or 0)
    except (TypeError, ValueError):
        return False, "备用地址未返回有效文件长度"
    if primary_total <= 0 or candidate_total <= 0:
        return False, "备用地址缺少可核对的文件长度"
    if primary_total != candidate_total:
        return False, f"备用地址文件长度不一致（{candidate_total} != {primary_total}）"

    primary_etag = _strong_etag(str(primary.get("etag") or ""))
    candidate_etag = _strong_etag(str(candidate.get("etag") or ""))
    if primary_etag and candidate_etag:
        if primary_etag == candidate_etag:
            return True, "etag"
        if not has_checksum:
            return False, "备用地址 ETag 与主地址不一致"

    primary_modified = str(primary.get("last_modified") or "")
    candidate_modified = str(candidate.get("last_modified") or "")
    if primary_modified and candidate_modified:
        if primary_modified == candidate_modified:
            return True, "last_modified"
        if not has_checksum and not (primary_etag and candidate_etag):
            return False, "备用地址 Last-Modified 与主地址不一致"

    if has_checksum:
        return True, "checksum"
    if primary.get("ranges") and candidate.get("ranges"):
        return True, "size_range"
    return False, "备用地址缺少可核对的文件身份"


def describe_mirror_state(state: str, detail: str = "") -> dict[str, str]:
    return {
        "state": str(state or "unknown")[:32],
        "detail": str(detail or "")[:300],
    }
