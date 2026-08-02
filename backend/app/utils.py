import json
import os
import re
from urllib.parse import (
    parse_qsl,
    unquote_plus,
    urlencode,
    urljoin,
    urlparse,
    urlsplit,
    urlunsplit,
)
from pathlib import Path


_VOLATILE_REQUEST_QUERY = re.compile(
    r"^(?:token|auth|authorization|signature|sig|expires?|expiry|policy|"
    r"key-pair-id|hdnea|hmac|jwt|session|sessionid|access[_-]?key|x-amz-.+|"
    r"_hls_(?:msn|part|skip))$",
    re.IGNORECASE,
)


def read_jsonl_prefix(path: str | Path) -> tuple[list[tuple[dict, int]], int]:
    """Read complete JSON-object lines and report their durable byte offsets.

    Journal appends always end in ``\n``. A final line without that terminator,
    invalid UTF-8/JSON, or a non-object value is a torn tail and must not hide
    later recovery work. Callers may apply their own schema checks and truncate
    to the last accepted offset with :func:`truncate_durable`.
    """
    source = Path(path)
    total_size = source.stat().st_size
    records: list[tuple[dict, int]] = []
    with source.open("rb") as stream:
        while True:
            line = stream.readline()
            if not line:
                break
            end_offset = stream.tell()
            if not line.endswith(b"\n"):
                break
            try:
                value = json.loads(line.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError):
                break
            if not isinstance(value, dict):
                break
            records.append((value, end_offset))
    return records, total_size


def truncate_durable(path: str | Path, size: int) -> None:
    """Durably discard an invalid append-journal tail."""
    with Path(path).open("r+b", buffering=0) as stream:
        stream.truncate(max(0, int(size)))
        os.fsync(stream.fileno())


def durable_replace(temporary: str | Path, destination: str | Path) -> None:
    """Flush a completed temporary file before atomically publishing it.

    A successful ``write`` only reaches the Windows page cache.  Live recorder
    checkpoints must never get ahead of the media files they describe, so
    force the bytes to stable storage before the rename makes the file visible
    as complete.
    """
    source = Path(temporary)
    target = Path(destination)
    with source.open("r+b", buffering=0) as stream:
        os.fsync(stream.fileno())
    source.replace(target)


def atomic_write_text(path: str | Path, content: str, *, encoding: str = "utf-8") -> None:
    """Durably replace a small state file without exposing a partial write.

    Long-running recorders depend on their checkpoint after a crash or power
    loss. Flush the temporary file to disk before the atomic rename so the
    JSON can never describe media bytes that were only buffered in Python.
    """
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(destination.name + ".tmp")
    try:
        with temporary.open("w", encoding=encoding) as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def stable_request_key(value: str, *, ignore_host: bool = False) -> str:
    """Identify a resource while ignoring only known short-lived signatures."""
    try:
        parsed = urlsplit(str(value or "").strip())
        pairs = parse_qsl(parsed.query, keep_blank_values=True)
        names = {name.lower() for name, _ in pairs}
        short_signature = "s" in names and "e" in names
        stable = [
            (name, item)
            for name, item in pairs
            if not _VOLATILE_REQUEST_QUERY.fullmatch(name)
            and not (short_signature and name.lower() in {"s", "e", "_t"})
        ]
        stable.sort(key=lambda item: (item[0].lower(), item[0], item[1]))
        path = parsed.path.rstrip("/") or "/"
        authority = "" if ignore_host else parsed.netloc.lower()
        return urlunsplit(
            (parsed.scheme.lower(), authority, path, urlencode(stable, doseq=True), "")
        )
    except (TypeError, ValueError):
        return str(value or "").split("#", 1)[0].rstrip("/")


def canonical_hls_url(value: str) -> str:
    """Remove LL-HLS cursors without re-encoding a signed query string."""
    try:
        parsed = urlsplit(str(value or "").strip())
        stable = []
        for raw_pair in parsed.query.split("&"):
            if not raw_pair:
                continue
            raw_name = raw_pair.partition("=")[0]
            if unquote_plus(raw_name).lower() in {
                "_hls_msn",
                "_hls_part",
                "_hls_skip",
            }:
                continue
            stable.append(raw_pair)
        return urlunsplit(
            (parsed.scheme, parsed.netloc, parsed.path, "&".join(stable), "")
        )
    except (TypeError, ValueError):
        return str(value or "")


def inherit_hls_access_query(base_url: str, resolved_url: str) -> str:
    """Carry known auth parameters to a same-origin relative HLS resource."""
    try:
        base = urlsplit(str(base_url or ""))
        child = urlsplit(str(resolved_url or ""))
        if (
            not base.query
            or child.query
            or base.scheme.lower() != child.scheme.lower()
            or base.netloc.lower() != child.netloc.lower()
        ):
            return str(resolved_url or "")
        raw_pairs = [pair for pair in base.query.split("&") if pair]
        names = {
            unquote_plus(pair.partition("=")[0]).lower()
            for pair in raw_pairs
        }
        short_signature = "s" in names and "e" in names
        inherited = []
        for pair in raw_pairs:
            name = unquote_plus(pair.partition("=")[0])
            lowered = name.lower()
            if lowered in {"_hls_msn", "_hls_part", "_hls_skip"}:
                continue
            if _VOLATILE_REQUEST_QUERY.fullmatch(name) or (
                short_signature and lowered in {"s", "e", "_t"}
            ):
                inherited.append(pair)
        if not inherited:
            return str(resolved_url or "")
        return urlunsplit(
            (child.scheme, child.netloc, child.path, "&".join(inherited), child.fragment)
        )
    except (TypeError, ValueError):
        return str(resolved_url or "")

def sanitize_filename(name: str) -> str:
    name = re.sub(r'[<>:"/\\|?*]', '_', name)
    name = re.sub(r'\s+', ' ', name).strip()
    name = name.strip('. ')
    if not name:
        name = "download"
    return name[:200]

def resolve_url(base: str, ref: str) -> str:
    return urljoin(base, ref)

def get_domain(url: str) -> str:
    parsed = urlparse(url)
    return parsed.hostname or ""

def safe_path(base_dir: str, filename: str) -> str:
    p = Path(base_dir) / sanitize_filename(filename)
    return str(p.resolve())

def humanize_bytes(n: float) -> str:
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if abs(n) < 1024:
            return f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.1f} PB"

def humanize_duration(seconds: float) -> str:
    if seconds < 0 or seconds > 360000:
        return "--:--:--"
    h = int(seconds // 3600)
    m = int((seconds % 3600) // 60)
    s = int(seconds % 60)
    return f"{h:02d}:{m:02d}:{s:02d}"
