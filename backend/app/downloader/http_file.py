from __future__ import annotations

import asyncio
import contextlib
import json
import mimetypes
import os
import re
import shutil
import time
from collections.abc import Callable, Coroutine
from collections import deque
from datetime import datetime
from pathlib import Path
from ..output_path import reserve_output_path
from dataclasses import dataclass
from typing import Any
from urllib.parse import parse_qsl, urlencode, unquote, unquote_to_bytes, urlparse, urlsplit, urlunsplit

import httpx

from ..config import settings
from ..checksum import apply_http_content_checksum, parse_http_content_checksum, prefer_http_content_checksum, verify_task_checksum
from ..models import Task, TaskStatus
from ..naming import is_generic_media_name
from ..request_context import build_task_headers, replay_request_body
from ..network_proxy import policy_httpx_client
from ..utils import atomic_write_text, sanitize_filename
from .engine import SeeklessEngine, publish_path, task_output_dir, task_work_dir
from .disk_space import MIN_FREE_RESERVE, ensure_download_capacity, ensure_free_space
from .errors import (
    MetadataProbeTimeout,
    SharedRetryWindow,
    _looks_like_signed_url,
    diagnose_download_error,
    format_download_error,
    redact_url,
    retry_delay_seconds,
    should_retry_download_error,
    should_share_retry_window,
)
from .throttle import throttle_bytes
from ..connection_parts import build_connection_parts, set_connection_parts
from .http_split import pick_endgame_split
from .response_validation import validate_download_response
from .mirrors import mirror_identity_compatible, normalize_mirror_urls


MAX_RETRIES = 5
PROBE_RESPONSE_TIMEOUT = 15.0
# The Range request and plain-GET fallback each have their own deadline. Keep
# an outer deadline too: a broken proxy/TLS close must never leave a task in
# "正在读取文件信息" forever.
PROBE_TOTAL_TIMEOUT = 35.0
PROBE_MAX_ATTEMPTS = 3
# Some CDN links are minted shortly before they become usable.  Waiting is
# intentionally bounded and only enabled for the complete, well-formed
# ``s/e/_t`` triplet; arbitrary future timestamps must fail normally.
SHORT_SIGNATURE_MAX_WAIT = 15 * 60
# Batch unbuffered range writes to cut syscall cost. Do not pass this size to
# httpx aiter_bytes(): ByteChunker holds a partial chunk until flush(), and a
# mid-stream ReadError never flushes, so the retry Range would restart at 0.
RANGE_WRITE_BATCH = 256 * 1024
_CONTENT_RANGE_RE = re.compile(
    r"^\s*(?:bytes\s+)?(?P<start>\d+)\s*-\s*(?P<end>\d+)\s*/\s*(?P<total>\d+|\*)\s*$",
    re.IGNORECASE,
)
_UNSATISFIED_RANGE_RE = re.compile(
    r"^\s*(?:bytes\s+)?\*\s*/\s*(?P<total>\d+)\s*$",
    re.IGNORECASE,
)
_VOLATILE_RESUME_QUERY = re.compile(
    r"^(?:token|auth|authorization|signature|sig|expires?|expiry|policy|"
    r"key-pair-id|hdnea|hmac|jwt|session|sessionid|access[_-]?key|x-amz-.+)$",
    re.IGNORECASE,
)
_MIME_EXTENSION_OVERRIDES = {
    "application/gzip": ".gz",
    "application/java-archive": ".jar",
    "application/pdf": ".pdf",
    "application/vnd.android.package-archive": ".apk",
    "application/x-7z-compressed": ".7z",
    "application/x-bzip2": ".bz2",
    "application/x-rar-compressed": ".rar",
    "application/x-tar": ".tar",
    "application/zip": ".zip",
    "audio/mp4": ".m4a",
    "audio/mpeg": ".mp3",
    "video/mp2t": ".ts",
    "video/mp4": ".mp4",
    "video/webm": ".webm",
}


class _HTTPRangeUnsupported(RuntimeError):
    """The origin stopped honoring byte ranges; restart safely as one stream."""


class _HTTPRangeValidationError(RuntimeError):
    """A partial response cannot be trusted to belong at the requested offset."""


def _parse_content_range(value: str) -> tuple[int, int, int | None] | None:
    """Parse common RFC 9110 and tolerant CDN Content-Range forms."""
    match = _CONTENT_RANGE_RE.match(str(value or ""))
    if not match:
        return None
    start = int(match.group("start"))
    end = int(match.group("end"))
    total_text = match.group("total")
    total = None if total_text == "*" else int(total_text)
    if end < start or (total is not None and (total <= 0 or end >= total)):
        return None
    return start, end, total


def _parse_unsatisfied_range(value: str) -> int | None:
    """Parse RFC 9110 unsatisfied-range Content-Range: bytes */N."""
    match = _UNSATISFIED_RANGE_RE.match(str(value or ""))
    if not match:
        return None
    total = int(match.group("total"))
    return total if total > 0 else None


def _ensure_filename_extension(filename: str, content_type: str) -> str:
    """Add a stable suffix when a generic download URL omitted one."""
    name = str(filename or "").strip()
    if not name or Path(name).suffix:
        return name
    mime = str(content_type or "").split(";", 1)[0].strip().lower()
    if not mime or mime in {"application/octet-stream", "binary/octet-stream"}:
        return name
    extension = _MIME_EXTENSION_OVERRIDES.get(mime) or mimetypes.guess_extension(mime, strict=False)
    if not extension or not re.fullmatch(r"\.[A-Za-z0-9]{1,10}", extension):
        return name
    return f"{name}{extension.lower()}"


def _response_decodes_content(response: httpx.Response) -> bool:
    encoding = response.headers.get("content-encoding", "").strip().lower()
    return bool(encoding and encoding != "identity")


def _metadata_probe_can_skip_body(
    *,
    content_type: str,
    final_url: str,
    server_filename: str,
    content_length: int,
) -> bool:
    """Whether response headers are strong enough to start a transfer.

    A few signed CDNs send headers immediately but hold the first body chunk
    until their edge has assembled a range.  Waiting for that chunk made a
    valid MP4 appear stuck in ``正在读取文件信息``.  We may proceed without a
    preview only when the server has already supplied a positive size and an
    unambiguous binary/media identity; HTML/JSON error pages still require a
    body prefix and are rejected by ``validate_download_response``.
    """
    if content_length <= 0:
        return False
    mime = str(content_type or "").split(";", 1)[0].strip().lower()
    if mime.startswith(("audio/", "video/", "image/")):
        return True
    if mime in {
        "application/octet-stream",
        "binary/octet-stream",
        "application/zip",
        "application/x-7z-compressed",
        "application/x-rar-compressed",
        "application/pdf",
    }:
        value = f"{server_filename} {final_url}".lower()
        return bool(re.search(r"\.(?:7z|zip|rar|pdf|mp3|m4a|flac|wav|mp4|mkv|webm|mov|avi|ts|m4s)(?:$|[?#\s])", value))
    # Unknown application types are not enough by themselves: a login/error
    # gateway is often mislabeled as ``application/octet-stream``.  An explicit
    # strong binary suffix can add the missing evidence without accepting a
    # generic JavaScript/XML response on headers alone.
    value = f"{server_filename} {final_url}".lower()
    return bool(re.search(
        r"\.(?:7z|apk|avi|bin|bz2|cab|dmg|docx?|exe|flac|gz|img|iso|jar|m4a|mkv|mov|mp3|mp4|msi|pdf|pptx?|rar|tar|tgz|torrent|wav|webm|whl|xlsx?|xz|zip)(?:$|[?#\s])",
        value,
    ))


def _short_signature_activation_delay(url: str, *, now: float | None = None) -> float:
    """Return seconds until a compact CDN signature becomes valid.

    ``mxcontent`` links carry ``s`` (signature), ``e`` (expiry) and ``_t``
    (not-before) as Unix seconds.  A future ``_t`` is not an expired URL and
    retrying immediately only creates a deterministic 403 loop.  We only wait
    when all fields are numeric, the window is coherent, the expiry is still
    ahead, and the delay is small enough to be useful to a user.
    """
    try:
        pairs = dict(parse_qsl(urlsplit(str(url or "")).query, keep_blank_values=True))
        if not {"s", "e", "_t"}.issubset(pairs):
            return 0.0
        not_before = int(pairs["_t"])
        expires = int(pairs["e"])
    except (TypeError, ValueError):
        return 0.0
    current = time.time() if now is None else float(now)
    if not_before >= expires or expires <= current:
        return 0.0
    delay = float(not_before) - current
    if delay <= 0 or delay > SHORT_SIGNATURE_MAX_WAIT:
        return 0.0
    return delay


def _strong_etag(value: str) -> str:
    etag = str(value or "").strip()
    return "" if etag.lower().startswith("w/") else etag


def _resume_resource_identity(value: str) -> str:
    """Bind partial bytes to one resource while ignoring expiring signatures."""
    try:
        parsed = urlsplit(str(value or ""))
        query = parse_qsl(parsed.query, keep_blank_values=True)
        names = {name.lower() for name, _value in query}
        short_signature = "s" in names and "e" in names
        stable = [
            (name, item)
            for name, item in query
            if not _VOLATILE_RESUME_QUERY.match(name)
            and not (short_signature and name.lower() in {"s", "e", "_t"})
        ]
        stable.sort(key=lambda item: (item[0].lower(), item[1]))
        return urlunsplit((
            parsed.scheme.lower(),
            parsed.netloc.lower(),
            parsed.path,
            urlencode(stable, doseq=True),
            "",
        ))
    except (TypeError, ValueError):
        return str(value or "").split("#", 1)[0]


class _SpeedWindow:
    """Rolling transfer rate over the last few seconds.

    An average since task start misleads twice: it counts bytes restored
    from a previous session as if they were just transferred, and it hides
    stalls long after they begin.  Only bytes that actually crossed the
    wire inside the window are counted here.
    """

    def __init__(self, span_seconds: float = 8.0) -> None:
        self._span = span_seconds
        self._samples: deque[tuple[float, int]] = deque()
        self._window_bytes = 0

    def _trim(self, now: float) -> None:
        cutoff = now - self._span
        while self._samples and self._samples[0][0] < cutoff:
            _, size = self._samples.popleft()
            self._window_bytes -= size

    def add(self, size: int) -> None:
        now = time.monotonic()
        self._samples.append((now, size))
        self._window_bytes += size
        self._trim(now)

    def speed(self) -> float:
        now = time.monotonic()
        self._trim(now)
        if not self._samples:
            return 0.0
        elapsed = max(now - self._samples[0][0], 0.25)
        return self._window_bytes / elapsed


def _reserve_output_path(path: Path) -> Path:
    return reserve_output_path(path)


def _content_disposition_filename(value: str) -> str:
    """Return a safe display name from legacy or RFC 5987 disposition fields."""
    if not value:
        return ""
    extended = re.search(
        r'(?:^|;)\s*filename\*\s*=\s*(?:"(?P<quoted>(?:\\.|[^"])*)"|(?P<plain>[^;]*))',
        value,
        re.IGNORECASE,
    )
    if extended:
        raw = (extended.group("quoted") if extended.group("quoted") is not None else extended.group("plain") or "").strip()
        parts = raw.split("'", 2)
        if len(parts) == 3:
            charset = parts[0].strip().lower().replace("utf8", "utf-8") or "utf-8"
            # RFC 5987 permits many charset labels, but accepting an arbitrary
            # codec here is unnecessary. Keep common browser/server labels and
            # make unknown labels deterministic rather than failing downloads.
            if charset not in {"utf-8", "iso-8859-1", "latin-1", "latin1", "us-ascii"}:
                charset = "utf-8"
            try:
                raw = unquote_to_bytes(parts[2]).decode(charset, errors="replace")
            except (LookupError, UnicodeError):
                raw = unquote(parts[2])
        else:
            raw = unquote(raw)
        return re.sub(r"[\x00-\x1f\x7f]", "", raw).strip()[:512]
    plain = re.search(
        r'(?:^|;)\s*filename\s*=\s*(?:"(?P<quoted>(?:\\.|[^"])*)"|(?P<plain>[^;]*))',
        value,
        re.IGNORECASE,
    )
    if plain:
        raw = plain.group("quoted") if plain.group("quoted") is not None else plain.group("plain") or ""
        raw = re.sub(r"\\(.)", r"\1", raw) if plain.group("quoted") is not None else raw
        return re.sub(r"[\x00-\x1f\x7f]", "", raw).strip()[:512]
    return ""


@dataclass
class _HttpSource:
    url: str
    final_url: str
    ranges: bool = True
    etag: str = ""
    last_modified: str = ""
    disabled: bool = False
    reason: str = ""


class HTTPDownloader(SeeklessEngine):
    def __init__(self, task: Task, on_progress=None, on_log=None) -> None:
        self.task = task
        self.on_progress = on_progress or (lambda task: None)
        self.on_log = on_log or (lambda task_id, message: None)
        self._priority_chunk: int | None = None
        self._priority_queue: asyncio.PriorityQueue | None = None
        self._completed_chunks: set[int] = set()
        self._claimed_chunks: set[int] = set()
        self._written_intervals: dict[int, int] = {}
        self._playback_fetcher: Callable[[int, int], Coroutine[Any, Any, None]] | None = None
        self._playback_fetch_tasks: dict[tuple[int, int], asyncio.Task] = {}
        self._chunk_size = max(1, int(settings.http_chunk_size_mb)) * 1024 * 1024
        self._part_path: Path | None = None
        self._total_size = 0
        self._sequential = False
        self._retry_window = SharedRetryWindow()
        self._last_rate_limit_notice = 0.0
        self._short_signature_waited = False
        # Data requests go to the redirect-resolved URL. Re-walking the
        # original redirect chain (github.com -> objects.githubusercontent.com
        # and similar) for every byte range adds a slow round trip per chunk
        # and often trips the origin's rate limiter.
        self._download_url = task.url
        self._url_generation = 0
        self._url_refresh_lock = asyncio.Lock()
        self._sources: list[_HttpSource] = []
        self._source_cursor = 0
        self._source_lock = asyncio.Lock()

    def _configured_mirrors(self) -> list[str]:
        return normalize_mirror_urls(self.task.url, (self.task.engine_state or {}).get("mirrors"))

    def _publish_mirror_status(self) -> None:
        status = []
        for source in self._sources:
            state = "failed" if source.disabled else "active"
            status.append({
                "url": source.url,
                "final_url": source.final_url,
                "state": state,
                "detail": source.reason,
                "ranges": bool(source.ranges),
            })
        self.task.engine_state["mirror_status"] = status

    def _install_source(self, metadata: dict, *, origin_url: str) -> _HttpSource:
        source = _HttpSource(
            url=origin_url or str(metadata.get("final_url") or self.task.url),
            final_url=str(metadata.get("final_url") or origin_url or self.task.url),
            ranges=bool(metadata.get("ranges")),
            etag=str(metadata.get("etag") or ""),
            last_modified=str(metadata.get("last_modified") or ""),
        )
        existing = next((item for item in self._sources if item.url == source.url or item.final_url == source.final_url), None)
        if existing is not None:
            existing.final_url = source.final_url
            existing.ranges = source.ranges
            existing.etag = source.etag
            existing.last_modified = source.last_modified
            existing.disabled = False
            existing.reason = ""
            source = existing
        else:
            self._sources.append(source)
        if not self._download_url or self._download_url == self.task.url:
            self._download_url = source.final_url
        self._publish_mirror_status()
        return source

    def _accept_mirror_metadata(self, origin_url: str, metadata: dict, primary: dict) -> tuple[bool, str]:
        ok, reason = mirror_identity_compatible(
            primary,
            metadata,
            has_checksum=bool(self.task.expected_checksum),
        )
        if ok:
            self._install_source(metadata, origin_url=origin_url)
        else:
            status = list(self.task.engine_state.get("mirror_status") or [])
            status.append({"url": origin_url, "final_url": str(metadata.get("final_url") or origin_url), "state": "skipped", "detail": reason, "ranges": bool(metadata.get("ranges"))})
            self.task.engine_state["mirror_status"] = status
        return ok, reason

    def _ensure_default_source(self, metadata: dict | None = None) -> None:
        if self._sources:
            return
        payload = dict(metadata or {})
        payload.setdefault("final_url", self._download_url or self.task.url)
        payload.setdefault("ranges", True)
        self._install_source(payload, origin_url=self.task.url)

    def _enabled_sources(self, *, require_ranges: bool = False) -> list[_HttpSource]:
        return [
            source
            for source in self._sources
            if not source.disabled and (not require_ranges or source.ranges)
        ]

    def _pick_source(self, *, require_ranges: bool = False) -> _HttpSource:
        self._ensure_default_source()
        candidates = self._enabled_sources(require_ranges=require_ranges)
        if not candidates:
            candidates = self._enabled_sources()
        if not candidates:
            return _HttpSource(url=self.task.url, final_url=self._download_url or self.task.url)
        source = candidates[self._source_cursor % len(candidates)]
        self._source_cursor += 1
        return source

    def _disable_source(self, source: _HttpSource, reason: str) -> None:
        source.disabled = True
        source.reason = reason[:300]
        self._publish_mirror_status()
        self.on_log(self.task.id, f"[downloading] 已停用地址 {redact_url(source.url)}：{reason}")
        remaining = self._enabled_sources()
        if remaining:
            self._download_url = remaining[0].final_url

    def _has_other_sources(self, source: _HttpSource, *, require_ranges: bool = False) -> bool:
        return any(item is not source for item in self._enabled_sources(require_ranges=require_ranges))

    async def _probe_metadata_with_failover(
        self,
        client: httpx.AsyncClient,
        headers: dict[str, str],
    ) -> dict:
        mirrors = self._configured_mirrors()
        primary_error: Exception | None = None
        try:
            metadata = await self._probe_with_retry(client, headers)
            self._install_source(metadata, origin_url=self.task.url)
            await self._discover_mirrors(client, headers, metadata)
            return metadata
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            primary_error = exc
            if not mirrors:
                raise
            self.on_log(
                self.task.id,
                f"[probing] 主地址失败，正在尝试 {len(mirrors)} 个备用地址：{exc}",
            )
        last_error = primary_error
        for mirror in mirrors:
            if self._is_canceled() or self._is_pausing():
                raise asyncio.CancelledError
            try:
                metadata = await asyncio.wait_for(self._probe(client, headers, url=mirror), timeout=PROBE_TOTAL_TIMEOUT)
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                last_error = exc
                status = list(self.task.engine_state.get("mirror_status") or [])
                status.append({"url": mirror, "final_url": mirror, "state": "failed", "detail": str(exc)[:300], "ranges": False})
                self.task.engine_state["mirror_status"] = status
                self.on_log(self.task.id, f"[probing] 备用地址不可用 {redact_url(mirror)}")
                continue
            self._install_source(metadata, origin_url=mirror)
            self._download_url = str(metadata.get("final_url") or mirror)
            self.on_log(self.task.id, f"[probing] 已切换到备用地址 {redact_url(mirror)}")
            remaining = [item for item in mirrors if item != mirror]
            if remaining:
                self.task.engine_state["mirrors"] = remaining
                await self._discover_mirrors(client, headers, metadata)
            return metadata
        if last_error is not None:
            raise last_error
        raise RuntimeError("服务器未返回可用的文件信息")

    async def _discover_mirrors(
        self,
        client: httpx.AsyncClient,
        headers: dict[str, str],
        primary: dict,
    ) -> None:
        for mirror in self._configured_mirrors():
            if self._is_canceled() or self._is_pausing():
                return
            if any(item.url == mirror or item.final_url == mirror for item in self._sources):
                continue
            try:
                metadata = await asyncio.wait_for(self._probe(client, headers, url=mirror), timeout=min(12.0, PROBE_TOTAL_TIMEOUT))
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                status = list(self.task.engine_state.get("mirror_status") or [])
                status.append({"url": mirror, "final_url": mirror, "state": "failed", "detail": str(exc)[:300], "ranges": False})
                self.task.engine_state["mirror_status"] = status
                self.on_log(self.task.id, f"[probing] 备用地址探测失败 {redact_url(mirror)}")
                continue
            ok, reason = self._accept_mirror_metadata(mirror, metadata, primary)
            if ok:
                self.on_log(self.task.id, f"[probing] 已启用备用地址 {redact_url(mirror)}（{reason}）")
            else:
                self.on_log(self.task.id, f"[probing] 已忽略备用地址 {redact_url(mirror)}：{reason}")

    def request_seek(self, value: int) -> None:
        if value >= 0:
            self._priority_chunk = int(value) // self._chunk_size
            if self._priority_queue is not None:
                self._priority_queue.put_nowait((-100, self._priority_chunk))

    def _range_is_available(self, start: int, end: int) -> bool:
        cursor = max(0, int(start))
        target = max(cursor, int(end)) + 1
        for interval_start, interval_end in sorted(self._written_intervals.items()):
            if interval_end <= cursor:
                continue
            if interval_start > cursor:
                return False
            cursor = max(cursor, interval_end)
            if cursor >= target:
                return True
        return False

    async def wait_for_range(self, start: int, end: int, timeout: float = 45.0) -> Path:
        if self._part_path is None:
            raise FileNotFoundError("下载临时文件尚未准备好")
        bounded_end = min(max(start, end), max(0, self._total_size - 1))
        if self._sequential:
            deadline = time.monotonic() + timeout
            while self.task.progress.downloaded_bytes <= bounded_end:
                if time.monotonic() >= deadline:
                    raise TimeoutError("目标字节范围尚未下载完成")
                await asyncio.sleep(0.1)
            return self._part_path
        first = start // self._chunk_size
        last = bounded_end // self._chunk_size
        required = set(range(first, last + 1))
        if self._priority_queue is not None:
            for order, index in enumerate(sorted(required)):
                self._priority_queue.put_nowait((-100 + order, index))
        key = (max(0, int(start)), bounded_end)
        priority_task = self._playback_fetch_tasks.get(key)
        if (
            not self._range_is_available(*key)
            and priority_task is None
            and self._playback_fetcher is not None
            and not self._is_canceled()
            and not self._is_pausing()
        ):
            priority_task = asyncio.create_task(
                self._playback_fetcher(*key),
                name=f"http-playback-range-{self.task.id}",
            )
            self._playback_fetch_tasks[key] = priority_task

            def forget(completed: asyncio.Task, request_key=key) -> None:
                if self._playback_fetch_tasks.get(request_key) is completed:
                    self._playback_fetch_tasks.pop(request_key, None)

            priority_task.add_done_callback(forget)
        deadline = time.monotonic() + timeout
        while not self._range_is_available(*key):
            # Older checkpoints only know complete chunks. Preserve that fast
            # path while exact intervals are populated for new transfers.
            if required.issubset(self._completed_chunks):
                return self._part_path
            if time.monotonic() >= deadline:
                raise TimeoutError("目标字节范围尚未下载完成")
            await asyncio.sleep(0.05)
        return self._part_path

    def _headers(self) -> dict[str, str]:
        headers = build_task_headers(self.task, browser_profile_managed=False)
        # Ranges and Content-Length describe the encoded representation. Ask
        # for identity so httpx's transparent decompression can never turn a
        # valid wire length into an apparently truncated/corrupt local file.
        headers["Accept-Encoding"] = "identity"
        return headers

    def _can_retry_with_browser_profile(self, error: BaseException) -> bool:
        """Whether a browser-originated 403 merits one curl-cffi retry.

        The normal HTTP engine deliberately uses httpx because it supports a
        high-quality parallel range transfer.  Some application download
        endpoints (notably authenticated attachment APIs) deliberately mask a
        missing browser session as ``404`` instead of returning ``401/403``.
        That is still a browser-profile failure, not proof that the file is
        absent.  HLS already uses curl-cffi for exactly this class of origin.
        Keep the fallback narrow: only the first direct GET probe, only a
        browser handoff, and only a 403 or a signed 404/410 with a captured
        browser context.
        """
        if self._is_replay_post():
            return False
        response = getattr(error, "response", None)
        status = int(getattr(response, "status_code", 0) or 0)
        if status == 403:
            return True
        if status not in {404, 410} or not _looks_like_signed_url(self.task.url):
            return False
        # A signed URL without any browser evidence is just a normal expired
        # link. Only replay it through curl-cffi when the extension supplied a
        # page identity, cookies, headers, or a scoped request context.
        return bool(
            self.task.source_page_url
            or self.task.cookie
            or self.task.referer
            or self.task.origin
            or self.task.request_headers
            or self.task.request_contexts
        )

    async def _download_with_browser_profile(
        self,
        part_path: Path,
        state_path: Path,
        task_dir: Path,
    ) -> bool:
        """Retry a rejected browser handoff with curl-cffi's Chrome profile.

        This intentionally remains a one-connection fallback.  The normal
        multi-range path stays untouched for every successful HTTP download;
        only a server that rejected the regular client gives up that parallel
        optimization in exchange for a coherent browser TLS/header profile.
        """
        from .hls import CurlAsyncSession, _BrowserHLSClient, _close_response

        if CurlAsyncSession is None:
            return False
        task = self.task
        output: Path | None = None
        response = None
        self._sequential = True
        task.progress.total_segments = 1
        task.progress.completed_segments = 0
        task.progress.downloaded_bytes = 0
        task.progress.progress_percent = 0.0
        task.progress.max_workers = 1
        task.progress.connection_status = "connecting"
        self._set_stage(
            "probing",
            "标准客户端被拒绝，正在以浏览器兼容连接重新验证文件",
        )
        headers = build_task_headers(
            task,
            accept="",
            request_url=task.url,
            browser_profile_managed=True,
        )
        headers["Accept-Encoding"] = "identity"
        self._discard_untrusted_sequential_part(part_path)
        existing = part_path.stat().st_size if part_path.exists() else 0
        if existing > 0:
            headers["Range"] = f"bytes={existing}-"
            task.progress.downloaded_bytes = existing
            if task.progress.total_bytes:
                task.progress.progress_percent = min(100.0, existing * 100 / task.progress.total_bytes)
        try:
            async with _BrowserHLSClient(
                1,
                task.url,
                deny_private_networks=bool(task.engine_state.get("browser_originated")),
            ) as client:
                response = await client.get(task.url, headers=headers, stream=True)
                if int(getattr(response, "status_code", 0) or 0) >= 400:
                    response.raise_for_status()
                status_code = int(getattr(response, "status_code", 0) or 0)
                if existing > 0 and status_code == 200:
                    existing = 0
                    task.engine_state.pop("sequential_bytes", None)
                    task.progress.downloaded_bytes = 0
                    task.progress.progress_percent = 0.0
                final_url = str(getattr(response, "url", "") or task.url)
                content_type = str(response.headers.get("content-type", "")).split(";", 1)[0]
                encoding = str(response.headers.get("content-encoding") or "").strip().lower()
                total = int(response.headers.get("content-length", 0) or 0)
                if encoding and encoding != "identity":
                    total = 0
                if existing > 0 and total > 0:
                    total = existing + total
                task.mime_type = task.mime_type or content_type
                if total > 0:
                    task.progress.total_bytes = total
                    task.engine_state["total_size"] = total
                filename = (
                    _content_disposition_filename(response.headers.get("content-disposition", ""))
                    or Path(urlparse(final_url).path).name
                    or Path(urlparse(task.url).path).name
                    or task.id
                )
                filename = _ensure_filename_extension(filename, task.mime_type)
                requested_name = task.filename.strip()
                task.filename = sanitize_filename(
                    filename if not requested_name or is_generic_media_name(requested_name) else requested_name
                )
                output = _reserve_output_path(task_output_dir(task) / task.filename)
                task.engine_state["reserved_output_path"] = str(output)
                if total > 0:
                    await asyncio.to_thread(
                        ensure_download_capacity,
                        part_path,
                        output,
                        total,
                        current_size=existing,
                    )
                else:
                    await asyncio.to_thread(
                        ensure_free_space,
                        part_path,
                        MIN_FREE_RESERVE,
                        operation="下载临时盘",
                    )
                task.progress.connection_status = "running"
                self._set_stage("downloading", "浏览器兼容连接已建立，正在单连接下载")
                task.engine_state["stream_path"] = str(part_path)
                window = _SpeedWindow()
                first_chunk = True
                with part_path.open("ab" if existing > 0 else "wb") as stream:
                    try:
                        content = response.aiter_content(chunk_size=256 * 1024)
                    except TypeError:
                        content = response.aiter_content()
                    async for chunk in content:
                        if self._is_canceled():
                            if getattr(response, "quit_now", None):
                                response.quit_now.set()
                            raise asyncio.CancelledError
                        if self._is_pausing():
                            task.engine_state["sequential_bytes"] = task.progress.downloaded_bytes
                            task.status = TaskStatus.PAUSED
                            self._set_stage("paused", "已暂停，可继续下载")
                            return True
                        if first_chunk:
                            validate_download_response(
                                task,
                                content_type=content_type,
                                content_length=task.progress.total_bytes,
                                preview=bytes(chunk[:65536]),
                                final_url=final_url,
                                server_filename=filename,
                            )
                            first_chunk = False
                        if not chunk:
                            continue
                        await throttle_bytes(len(chunk), task)
                        stream.write(chunk)
                        task.progress.downloaded_bytes += len(chunk)
                        task.engine_state["sequential_bytes"] = task.progress.downloaded_bytes
                        window.add(len(chunk))
                        self._apply_speed(window)
                        self._publish()
            if not part_path.exists() or part_path.stat().st_size <= 0:
                raise RuntimeError("浏览器兼容连接没有返回文件数据")
            if task.progress.total_bytes and part_path.stat().st_size != task.progress.total_bytes:
                raise RuntimeError(
                    f"文件长度不匹配，期望 {task.progress.total_bytes}，实际 {part_path.stat().st_size}"
                )
            task.progress.completed_segments = 1
            self._set_stage("verifying", "浏览器兼容下载完成，正在写入并校验最终文件")
            output = self._refine_output_extension(output)
            await asyncio.to_thread(publish_path, part_path, output)
            state_path.unlink(missing_ok=True)
            task.output_path = str(output)
            task.engine_state["output_is_file"] = True
            task.engine_state.pop("reserved_output_path", None)
            task.engine_state["stream_path"] = str(output)
            task.engine_state["total_size"] = output.stat().st_size
            if not await verify_task_checksum(task, output, on_progress=self.on_progress, on_log=self.on_log):
                return True
            task.status = TaskStatus.DONE
            task.finished_at = datetime.now().isoformat()
            task.progress.progress_percent = 100.0
            task.progress.connection_status = "idle"
            self._set_stage("done", f"完成: {output.name}")
            if not settings.keep_temp_files:
                await asyncio.to_thread(shutil.rmtree, task_dir, True)
            return True
        finally:
            if response is not None:
                await _close_response(response)
            if output and task.status is not TaskStatus.DONE and output.exists() and output.stat().st_size == 0:
                output.unlink(missing_ok=True)

    def _is_replay_post(self) -> bool:
        return str(self.task.request_method or "GET").upper() == "POST"

    def _publish(self) -> None:
        self.on_progress(self.task)

    def _refine_output_extension(self, output: Path) -> Path:
        """Rename an empty reservation after the transfer reveals its MIME type.

        A surprising number of signed/CDN URLs have no useful suffix and do
        not return Content-Type on the metadata range request.  Keep the
        early reservation (it prevents name races), but replace that empty
        placeholder before publishing once the real response identifies a
        safe extension.
        """
        filename = _ensure_filename_extension(self.task.filename, self.task.mime_type)
        if not filename or filename == self.task.filename:
            return output
        replacement = _reserve_output_path(
            task_output_dir(self.task) / sanitize_filename(filename)
        )
        if output.exists() and output.stat().st_size == 0:
            output.unlink(missing_ok=True)
        self.task.filename = replacement.name
        self.task.engine_state["reserved_output_path"] = str(replacement)
        return replacement

    def _apply_speed(self, window: _SpeedWindow) -> None:
        progress = self.task.progress
        speed = window.speed()
        progress.speed_bytes_per_sec = speed
        total = progress.total_bytes
        if total:
            progress.progress_percent = min(
                100.0, progress.downloaded_bytes * 100 / total
            )
            remaining = max(0, total - progress.downloaded_bytes)
            progress.eta_seconds = remaining / speed if speed > 0 else 0.0
        else:
            progress.eta_seconds = 0.0

    def _set_stage(self, stage: str, message: str) -> None:
        self.task.stage = stage
        self.task.last_log = message
        self.on_log(self.task.id, f"[{stage}] {message}")
        self._publish()

    async def _wait_for_short_signature_activation(self) -> None:
        """Wait briefly for a verified mxcontent not-before timestamp."""
        delay = _short_signature_activation_delay(self.task.url)
        if delay <= 0:
            return
        deadline = time.monotonic() + delay
        next_notice = 0.0
        while True:
            if self._is_canceled() or self._is_pausing():
                raise asyncio.CancelledError
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            now = time.monotonic()
            if now >= next_notice:
                seconds = max(1, int(remaining + 0.999))
                self._set_stage("probing", f"短效签名尚未生效，等待约 {seconds} 秒")
                next_notice = now + 5.0
            await asyncio.sleep(min(1.0, remaining))
        self._set_stage("probing", "短效签名已到可用时间，正在读取文件信息")

    def _is_canceled(self) -> bool:
        return bool(self.task.cancel_event and self.task.cancel_event.is_set())

    def _is_pausing(self) -> bool:
        return bool(self.task.pause_event and self.task.pause_event.is_set())

    def _announce_rate_limit(self, remaining: float) -> None:
        """Expose one concise task-level signal for a shared server cooldown."""
        now = time.monotonic()
        if now - self._last_rate_limit_notice < 0.5:
            return
        self._last_rate_limit_notice = now
        seconds = max(1, int(remaining + 0.999))
        self.task.progress.connection_status = "rate_limited"
        self._set_stage("downloading", f"服务器限流，所有分片等待约 {seconds} 秒")

    def _clear_rate_limit_notice(self) -> None:
        if self.task.progress.connection_status != "rate_limited":
            return
        self.task.progress.connection_status = "running"
        self._set_stage("downloading", "服务器限流结束，继续分段下载")

    async def _probe(self, client: httpx.AsyncClient, headers: dict[str, str], url: str | None = None) -> dict:
        target_url = url or self.task.url
        fallback: httpx.Response | None = None
        ranged: httpx.Response | None = None
        try:
            # Probe with a small GET range first.  Real-world download servers
            # frequently leave HEAD hanging, reject it, or return metadata that
            # differs from GET.  Mature download managers therefore test the
            # actual transfer path and fall back to a streamed plain GET.
            try:
                request = client.build_request(
                    "GET",
                    target_url,
                    headers={**headers, "Range": "bytes=0-255"},
                )
                ranged = await asyncio.wait_for(
                    client.send(request, stream=True),
                    timeout=PROBE_RESPONSE_TIMEOUT,
                )
                assert ranged is not None
                # Most HTTP rejections are authoritative; only errors commonly
                # caused by the Range form are eligible for a plain-GET retry.
                ranged.raise_for_status()
            except (httpx.TransportError, httpx.HTTPStatusError, asyncio.TimeoutError) as exc:
                status = (
                    exc.response.status_code
                    if isinstance(exc, httpx.HTTPStatusError)
                    else 0
                )
                # These statuses commonly mean only that the small Range form
                # was rejected. Authentication, expiry, missing files, rate
                # limits and server errors are real download failures.
                if status and status not in {400, 405, 416}:
                    raise
                if ranged is not None:
                    await ranged.aclose()
                    ranged = None
                try:
                    request = client.build_request("GET", target_url, headers=headers)
                    fallback = await asyncio.wait_for(
                        client.send(request, stream=True),
                        timeout=PROBE_RESPONSE_TIMEOUT,
                    )
                    assert fallback is not None
                    fallback.raise_for_status()
                except Exception:
                    if fallback is not None:
                        await fallback.aclose()
                        fallback = None
                    raise

            response = ranged or fallback
            if response is None:
                raise RuntimeError("服务器未返回可用的文件信息")
            response.raise_for_status()
            content_range = _parse_content_range(response.headers.get("content-range", ""))
            encoded = _response_decodes_content(response)
            range_supported = bool(
                ranged is not None
                and ranged.status_code == 206
                and content_range
                and content_range[0] == 0
                and content_range[2] is not None
                and not encoded
            )
            # A 206 response with ``Content-Range: */*`` (or a malformed
            # range header) only describes the short probe body. Treating its
            # Content-Length as the object size makes a large MP4 appear to
            # be 256 bytes and leaves the task stuck at the final verify step.
            partial_probe_without_total = bool(
                ranged is not None
                and ranged.status_code == 206
                and not range_supported
            )
            range_total = content_range[2] if range_supported and content_range else None
            total = (
                int(range_total)
                if range_total is not None
                else 0
                if partial_probe_without_total
                else int(
                    response.headers.get("content-length", 0)
                    or (fallback.headers.get("content-length", 0) if fallback else 0)
                    or 0
                )
            )
            if encoded:
                # aiter_bytes yields the decoded entity, so the encoded wire
                # length cannot be used for allocation or final validation.
                total = 0
                range_supported = False
            filename = _content_disposition_filename(response.headers.get("content-disposition", ""))
            if not filename and fallback is not None:
                filename = _content_disposition_filename(fallback.headers.get("content-disposition", ""))
            content_type = (response.headers.get("content-type", "") or (fallback.headers.get("content-type", "") if fallback else "")).split(";", 1)[0]
            server_filename = filename
            preview = b""
            # Read only a bounded first chunk for the content validation.  Do
            # not let a CDN that has sent reliable metadata but is slow to
            # release bytes hold the task in the metadata stage indefinitely.
            # The outer probe deadline still protects ambiguous responses.
            stream = response.aiter_bytes()
            try:
                preview = bytes(await asyncio.wait_for(
                    stream.__anext__(),
                    timeout=min(3.0, PROBE_RESPONSE_TIMEOUT),
                ))[:65536]
            except StopAsyncIteration:
                preview = b""
            except asyncio.TimeoutError:
                if not _metadata_probe_can_skip_body(
                    content_type=content_type,
                    final_url=str(response.url),
                    server_filename=server_filename,
                    content_length=total,
                ):
                    raise
                self.on_log(
                    self.task.id,
                    "[probing] 服务器已返回可靠文件信息，首个数据块较慢，继续下载",
                )
            validate_download_response(
                self.task,
                content_type=content_type,
                content_length=total,
                preview=preview,
                final_url=str(response.url),
                server_filename=server_filename,
            )
            checksum = prefer_http_content_checksum(
                parse_http_content_checksum(response.headers),
                parse_http_content_checksum(fallback.headers) if fallback is not None else "",
            )
            if checksum:
                apply_http_content_checksum(self.task, checksum=checksum)
            return {
                "total": total,
                "ranges": range_supported,
                "etag": response.headers.get("etag", "") or (fallback.headers.get("etag", "") if fallback else ""),
                "last_modified": response.headers.get("last-modified", "") or (fallback.headers.get("last-modified", "") if fallback else ""),
                "content_type": content_type,
                "filename": filename,
                "final_url": str(response.url),
                "checksum": checksum,
            }
        finally:
            if ranged is not None:
                await ranged.aclose()
            if fallback is not None:
                await fallback.aclose()

    async def _probe_with_retry(
        self,
        client: httpx.AsyncClient,
        headers: dict[str, str],
        url: str | None = None,
    ) -> dict:
        """Probe through transient CDN failures without hiding permanent errors."""
        if not self._short_signature_waited:
            self._short_signature_waited = True
            delay = _short_signature_activation_delay(url or self.task.url)
            if delay > 0:
                self.task.progress.connection_status = "waiting"
                self._set_stage(
                    "probing",
                    f"短效链接尚未生效，等待约 {int(delay + 0.999)} 秒后再读取文件信息",
                )
                deadline = time.monotonic() + delay
                while True:
                    if self._is_canceled() or self._is_pausing():
                        raise asyncio.CancelledError
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        break
                    await asyncio.sleep(min(0.25, remaining))
                self.task.progress.connection_status = "connecting"
                self._set_stage("probing", "短效链接已生效，正在读取文件信息")
        last_error: Exception | None = None
        for attempt in range(1, PROBE_MAX_ATTEMPTS + 1):
            if self._is_canceled() or self._is_pausing():
                raise asyncio.CancelledError
            try:
                return await asyncio.wait_for(
                    self._probe(client, headers, url) if url else self._probe(client, headers),
                    timeout=PROBE_TOTAL_TIMEOUT,
                )
            except asyncio.TimeoutError as exc:
                last_error = MetadataProbeTimeout(
                    f"文件信息探测超过 {int(PROBE_TOTAL_TIMEOUT)} 秒"
                )
                last_error.__cause__ = exc
            except Exception as exc:
                last_error = exc

            if (
                last_error is None
                or not should_retry_download_error(last_error)
                or attempt >= PROBE_MAX_ATTEMPTS
            ):
                break
            task = self.task
            task.progress.reconnect_count += 1
            task.progress.connection_status = "reconnecting"
            delay = retry_delay_seconds(last_error, attempt)
            self._set_stage(
                "probing",
                f"读取文件信息暂时失败，{delay:g} 秒后自动重试（{attempt}/{PROBE_MAX_ATTEMPTS - 1}）",
            )
            await self._retry_window.extend(delay)
            if not await self._retry_window.wait(
                lambda: self._is_canceled() or self._is_pausing()
            ):
                raise asyncio.CancelledError

        if last_error is not None:
            raise last_error
        raise RuntimeError("服务器未返回可用的文件信息")

    async def _refresh_download_url(
        self,
        client: httpx.AsyncClient,
        headers: dict[str, str],
        *,
        generation: int,
    ) -> bool:
        """Re-resolve an expired redirect target from the original task URL.

        Signed CDN redirects (GitHub releases, S3 presigned links) expire in
        minutes while a large transfer is still running.  One worker probes
        the original URL again; the others simply retry with the refreshed
        address instead of stacking duplicate probes.
        """
        async with self._url_refresh_lock:
            if self._url_generation != generation:
                return True
            if self._download_url == self.task.url:
                return False
            try:
                metadata = await asyncio.wait_for(
                    self._probe(client, headers),
                    timeout=PROBE_TOTAL_TIMEOUT,
                )
            except Exception:
                return False
            total = int(metadata.get("total") or 0)
            if self._total_size and total and total != self._total_size:
                # A different object now lives behind the URL. Let the range
                # validators fail the transfer safely instead of stitching.
                return False
            self._download_url = str(metadata.get("final_url") or "") or self.task.url
            if self._sources:
                current = next((item for item in self._sources if not item.disabled), self._sources[0])
                current.final_url = self._download_url
                self._publish_mirror_status()
            self._url_generation += 1
            self.on_log(
                self.task.id,
                "[downloading] 跳转后的下载地址已过期，已从原始链接重新解析并继续",
            )
            return True

    async def _download_replay_post(
        self,
        client: httpx.AsyncClient,
        headers: dict[str, str],
        part_path: Path,
    ) -> Path:
        """Download a captured POST response exactly once.

        Replaying a form/API request for every range would duplicate side
        effects (and frequently invalidates one-time links). It therefore uses
        one streaming POST with no metadata probe, Range or resume reuse.
        """
        task = self.task
        body = replay_request_body(task.request_method, task.request_body, task.request_headers)
        if not body:
            raise RuntimeError("浏览器 POST 请求体无效或不允许重放")
        self._sequential = True
        task.progress.total_segments = 1
        task.progress.max_workers = 1
        task.progress.connection_status = "running"
        task.progress.active_workers = 1
        task.progress.active_slots = 1
        task.progress.downloaded_bytes = 0
        task.progress.total_bytes = 0
        task.progress.progress_percent = 0.0
        set_connection_parts(task, [])
        self._set_stage("downloading", "正在安全重放浏览器 POST 下载（单连接）")
        task.engine_state["stream_path"] = str(part_path)
        task.engine_state["post_replay"] = True
        window = _SpeedWindow()
        async with client.stream("POST", task.url, headers=headers, content=body) as response:
            response.raise_for_status()
            task.mime_type = task.mime_type or response.headers.get("content-type", "").split(";", 1)[0]
            task.progress.total_bytes = int(response.headers.get("content-length", 0) or 0)
            task.engine_state["total_size"] = task.progress.total_bytes
            filename = (
                _content_disposition_filename(response.headers.get("content-disposition", ""))
                or Path(urlparse(str(response.url)).path).name
                or Path(urlparse(task.url).path).name
                or task.id
            )
            filename = _ensure_filename_extension(filename, task.mime_type)
            requested_name = task.filename.strip()
            task.filename = sanitize_filename(filename if not requested_name or is_generic_media_name(requested_name) else requested_name)
            output = _reserve_output_path(task_output_dir(task) / task.filename)
            task.engine_state["reserved_output_path"] = str(output)
            with part_path.open("wb") as stream:
                first_chunk = True
                async for chunk in response.aiter_bytes():
                    if first_chunk:
                        validate_download_response(
                            task,
                            content_type=response.headers.get("content-type", ""),
                            content_length=task.progress.total_bytes,
                            preview=bytes(chunk[:65536]),
                            final_url=str(response.url),
                            server_filename=filename,
                        )
                        first_chunk = False
                    if self._is_canceled():
                        raise asyncio.CancelledError
                    if self._is_pausing():
                        return output
                    await throttle_bytes(len(chunk), task)
                    stream.write(chunk)
                    task.progress.downloaded_bytes += len(chunk)
                    window.add(len(chunk))
                    self._apply_speed(window)
                    self._publish()
        task.progress.completed_segments = 1
        return output

    async def run(self) -> None:
        task = self.task
        # Older databases and hand-edited task payloads may still contain the
        # historical sentinel 0.  A zero-sized worker pool leaves a valid
        # ranged download permanently in "准备下载" with no exception.
        task.concurrency = min(
            64,
            max(1, int(task.concurrency or settings.default_concurrency or 12)),
        )
        task_dir = task_work_dir(task)
        task_dir.mkdir(parents=True, exist_ok=True)
        part_path = task_dir / "payload.downloading"
        self._part_path = part_path
        state_path = task_dir / "http-resume.json"
        output: Path | None = None
        try:
            task.started_at = task.started_at or datetime.now().isoformat()
            task.status = TaskStatus.DOWNLOADING
            task.progress.connection_status = "connecting"
            self._set_stage("probing", "正在读取文件信息")
            # Leave headroom above the worker count: redirect hops, the probe
            # and playback range requests share this pool, and a pool sized
            # exactly to the workers serializes chunk startup behind them.
            limits = httpx.Limits(
                max_connections=max(8, task.concurrency * 2),
                max_keepalive_connections=max(8, task.concurrency * 2),
            )
            timeout = httpx.Timeout(connect=15, read=60, write=30, pool=30)
            headers = self._headers()
            async with policy_httpx_client(
                follow_redirects=True,
                timeout=timeout,
                limits=limits,
                deny_private_networks=bool(task.engine_state.get("browser_originated")),
            ) as client:
                if self._is_replay_post():
                    output = await self._download_replay_post(client, headers, part_path)
                else:
                    await self._wait_for_short_signature_activation()
                    metadata = await self._probe_metadata_with_failover(client, headers)
                    total = int(metadata["total"])
                    self._total_size = total
                    self._download_url = str(metadata.get("final_url") or "") or task.url
                    self._ensure_default_source(metadata)
                    task.mime_type = task.mime_type or metadata["content_type"]
                    task.progress.total_bytes = total
                    name = (
                        metadata["filename"]
                        or Path(urlparse(metadata.get("final_url", "")).path).name
                        or Path(urlparse(task.url).path).name
                        or task.id
                    )
                    name = _ensure_filename_extension(name, task.mime_type)
                    requested_name = task.filename.strip()
                    task.filename = sanitize_filename(name if not requested_name or is_generic_media_name(requested_name) else requested_name)
                    output = _reserve_output_path(task_output_dir(task) / task.filename)
                    task.engine_state["reserved_output_path"] = str(output)

                    current_size = part_path.stat().st_size if part_path.exists() else 0
                    if total > 0:
                        await asyncio.to_thread(
                            ensure_download_capacity,
                            part_path,
                            output,
                            total,
                            current_size=current_size,
                        )
                    else:
                        await asyncio.to_thread(
                            ensure_free_space,
                            part_path,
                            MIN_FREE_RESERVE,
                            operation="下载临时盘",
                        )

                    marked = int(task.engine_state.get("sequential_bytes") or 0)
                    trusted_sequential = marked > 0 and current_size == marked
                    if total <= 0 or not metadata["ranges"] or (trusted_sequential and (total <= 0 or current_size < total)):
                        self._sequential = True
                        self._discard_untrusted_sequential_part(part_path)
                        await self._download_sequential(client, headers, part_path)
                    else:
                        try:
                            await self._download_ranges(client, headers, part_path, state_path, metadata)
                        except _HTTPRangeUnsupported:
                            # A CDN can advertise 206 during probing and later
                            # ignore Range or fail If-Range after an object
                            # rotation. Never stitch its full 200 response into
                            # sparse offsets: discard the range checkpoint, the
                            # preallocated sparse part, and restart one verified
                            # sequential transfer from byte 0.
                            self._sequential = True
                            self._completed_chunks.clear()
                            self._claimed_chunks.clear()
                            state_path.unlink(missing_ok=True)
                            part_path.unlink(missing_ok=True)
                            task.engine_state.pop("sequential_bytes", None)
                            task.progress.completed_segments = 0
                            task.progress.downloaded_bytes = 0
                            task.progress.progress_percent = 0.0
                            self._set_stage(
                                "downloading",
                                "服务器已停止支持分段，正在自动切换单连接并从头安全下载",
                            )
                            await self._download_sequential(client, headers, part_path)

            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                task.finished_at = datetime.now().isoformat()
                self._set_stage("canceled", "已取消")
                return
            if self._is_pausing():
                task.status = TaskStatus.PAUSED
                self._set_stage("paused", "已暂停，可继续下载")
                return
            if output is None:
                raise RuntimeError("下载输出路径未初始化")
            output = self._refine_output_extension(output)
            if not part_path.exists() or part_path.stat().st_size <= 0:
                raise RuntimeError("下载结果为空")
            if task.progress.total_bytes and part_path.stat().st_size != task.progress.total_bytes:
                raise RuntimeError(
                    f"文件长度不匹配，期望 {task.progress.total_bytes}，实际 {part_path.stat().st_size}"
                )
            self._set_stage("verifying", "下载完成，正在写入并校验最终文件")
            await asyncio.to_thread(publish_path, part_path, output)
            state_path.unlink(missing_ok=True)
            task.output_path = str(output)
            task.engine_state["output_is_file"] = True
            task.engine_state.pop("reserved_output_path", None)
            task.engine_state["stream_path"] = str(output)
            task.engine_state["total_size"] = output.stat().st_size
            if not await verify_task_checksum(task, output, on_progress=self.on_progress, on_log=self.on_log):
                return
            task.status = TaskStatus.DONE
            task.finished_at = datetime.now().isoformat()
            task.progress.progress_percent = 100.0
            task.progress.connection_status = "idle"
            self._set_stage("done", f"完成: {output.name}")
            if not settings.keep_temp_files:
                await asyncio.to_thread(shutil.rmtree, task_dir, True)
        except asyncio.CancelledError:
            task.progress.connection_status = "idle"
            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                task.finished_at = datetime.now().isoformat()
                if not settings.keep_temp_files:
                    await asyncio.to_thread(shutil.rmtree, task_dir, True)
            else:
                task.status = TaskStatus.PAUSED
                task.stage = "interrupted"
                task.last_log = "程序已关闭，临时文件已保留，可恢复"
                self._publish()
            if output and output.exists() and output.stat().st_size == 0:
                output.unlink(missing_ok=True)
            raise
        except Exception as exc:
            if output is None and self._can_retry_with_browser_profile(exc):
                status = int(getattr(getattr(exc, "response", None), "status_code", 0) or 0)
                self.on_log(
                    task.id,
                    f"[probing] 标准 HTTP 请求收到 {status or '拒绝'}，正在尝试浏览器兼容 TLS 指纹回退",
                )
                try:
                    if await self._download_with_browser_profile(part_path, state_path, task_dir):
                        return
                except Exception as fallback_error:
                    # Preserve the original, user-actionable 403 diagnosis if
                    # the browser-profile retry is rejected too (for example
                    # a genuinely expired one-time URL). The fallback reason
                    # stays in the task log without exposing credentials.
                    self.on_log(
                        task.id,
                        f"[probing] 浏览器兼容回退未通过: {type(fallback_error).__name__}",
                    )
            details = diagnose_download_error(exc, stage=task.stage, url=task.url, task_context=task)
            task.error_code = details.code
            task.error_stage = details.stage
            task.error_url = details.url
            task.error_hint = details.hint
            task.http_status = details.http_status
            task.error_message = format_download_error(details)
            task.status = TaskStatus.FAILED
            task.finished_at = datetime.now().isoformat()
            task.progress.connection_status = "error"
            self._set_stage("failed", task.error_message)
            if output and output.exists() and output.stat().st_size == 0:
                output.unlink(missing_ok=True)
        finally:
            if (
                output
                and task.status is not TaskStatus.DONE
                and output.exists()
                and output.stat().st_size == 0
            ):
                output.unlink(missing_ok=True)

    def _discard_untrusted_sequential_part(self, part_path: Path) -> None:
        """Drop a Range-preallocated sparse file before sequential restart.

        Multi-connection downloads truncate the part to Content-Length with
        zeros. A shorter unmarked file is a sequential prefix and must be
        kept so pause/resume from older builds still continues with Range.
        """
        if not part_path.exists():
            return
        existing = part_path.stat().st_size
        marked = int(self.task.engine_state.get("sequential_bytes") or 0)
        if marked > 0 and existing == marked:
            return
        total = int(self.task.progress.total_bytes or self._total_size or 0)
        if total > 0 and existing >= total:
            part_path.unlink(missing_ok=True)
            self.task.engine_state.pop("sequential_bytes", None)

    async def _download_sequential(
        self,
        client: httpx.AsyncClient,
        headers: dict[str, str],
        part_path: Path,
    ) -> None:
        task = self.task
        task.progress.total_segments = 1
        task.progress.max_workers = 1
        task.progress.connection_status = "running"
        task.progress.active_workers = 1
        task.progress.active_slots = 1
        set_connection_parts(task, [])
        self._set_stage("downloading", "服务器不支持分段，正在单连接下载")
        task.engine_state["stream_path"] = str(part_path)
        task.engine_state["total_size"] = task.progress.total_bytes
        window = _SpeedWindow()
        last_error: Exception | None = None
        for attempt in range(1, MAX_RETRIES + 1):
            if self._is_canceled():
                raise asyncio.CancelledError
            if self._is_pausing():
                return
            url_generation = self._url_generation
            try:
                source = self._pick_source()
                existing = part_path.stat().st_size if part_path.exists() else 0
                request_headers = dict(headers)
                if existing > 0:
                    request_headers["Range"] = f"bytes={existing}-"
                    request_headers["Accept-Encoding"] = "identity"
                async with client.stream("GET", source.final_url or self._download_url, headers=request_headers) as response:
                    if existing > 0 and response.status_code == 416:
                        unsatisfied = _parse_unsatisfied_range(response.headers.get("content-range", ""))
                        known_total = int(task.progress.total_bytes or self._total_size or unsatisfied or 0)
                        marked = int(task.engine_state.get("sequential_bytes") or 0)
                        trusted_complete = bool(
                            known_total
                            and existing >= known_total
                            and marked > 0
                            and marked >= min(existing, known_total)
                        )
                        if trusted_complete:
                            task.progress.downloaded_bytes = existing
                            task.progress.completed_segments = 1
                            if known_total:
                                task.progress.total_bytes = known_total
                            return
                        existing = 0
                        part_path.unlink(missing_ok=True)
                        task.engine_state.pop("sequential_bytes", None)
                        continue
                    append = existing > 0 and response.status_code == 206
                    if existing > 0 and response.status_code == 200:
                        existing = 0
                        append = False
                        task.engine_state.pop("sequential_bytes", None)
                    elif not append:
                        response.raise_for_status()
                    content_type = response.headers.get("content-type", "").split(";", 1)[0]
                    task.mime_type = task.mime_type or content_type
                    reported_total = 0
                    if append:
                        content_range = _parse_content_range(response.headers.get("content-range", ""))
                        if content_range is None or content_range[0] != existing:
                            raise _HTTPRangeValidationError("Range 续传响应与本地已下载偏移不一致")
                        reported_total = int(content_range[2] or 0) or int(
                            task.progress.total_bytes or self._total_size or 0
                        )
                    else:
                        reported_total = int(response.headers.get("content-length", 0) or 0)
                        if _response_decodes_content(response):
                            reported_total = 0
                    if reported_total:
                        task.progress.total_bytes = reported_total
                        self._total_size = reported_total
                        task.engine_state["total_size"] = reported_total
                    task.progress.downloaded_bytes = existing
                    if reported_total:
                        task.progress.progress_percent = min(100.0, existing * 100 / reported_total)
                    with part_path.open("ab" if append else "wb") as output:
                        first_chunk = True
                        async for chunk in response.aiter_bytes():
                            if first_chunk:
                                validate_download_response(
                                    task,
                                    content_type=content_type,
                                    content_length=reported_total,
                                    preview=bytes(chunk[:65536]),
                                    final_url=str(response.url),
                                )
                                first_chunk = False
                            if self._is_canceled():
                                raise asyncio.CancelledError
                            if self._is_pausing():
                                return
                            await throttle_bytes(len(chunk), task)
                            output.write(chunk)
                            task.progress.downloaded_bytes += len(chunk)
                            task.engine_state["sequential_bytes"] = task.progress.downloaded_bytes
                            window.add(len(chunk))
                            self._apply_speed(window)
                            self._publish()
                if task.progress.total_bytes and task.progress.downloaded_bytes != task.progress.total_bytes:
                    raise httpx.RemoteProtocolError(
                        f"响应提前结束，期望 {task.progress.total_bytes} 字节，实际 {task.progress.downloaded_bytes} 字节"
                    )
                task.progress.completed_segments = 1
                return
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                last_error = exc
                if self._is_pausing():
                    return
                status = int(
                    getattr(getattr(exc, "response", None), "status_code", 0) or 0
                )
                if status in {401, 403, 404, 410} and self._has_other_sources(source):
                    self._disable_source(source, f"HTTP {status}")
                    continue
                if (
                    status in {401, 403, 404, 410}
                    and attempt < MAX_RETRIES
                    and await self._refresh_download_url(
                        client, headers, generation=url_generation
                    )
                ):
                    continue
                if not should_retry_download_error(exc) or attempt >= MAX_RETRIES:
                    break
                delay = retry_delay_seconds(exc, min(4, attempt))
                self._set_stage(
                    "downloading",
                    f"单连接传输中断，{delay:g} 秒后续传重试（{attempt}/{MAX_RETRIES - 1}）",
                )
                await asyncio.sleep(delay)
        if last_error is not None:
            raise last_error

    async def _download_ranges(
        self,
        client: httpx.AsyncClient,
        headers: dict[str, str],
        part_path: Path,
        state_path: Path,
        metadata: dict,
    ) -> None:
        task = self.task
        self._ensure_default_source(metadata)
        total = int(metadata["total"])
        chunk_size = max(1, int(settings.http_chunk_size_mb)) * 1024 * 1024
        self._chunk_size = chunk_size
        chunks = [(start, min(total - 1, start + chunk_size - 1)) for start in range(0, total, chunk_size)]
        completed: set[int] = set()
        range_current = {index: start for index, (start, _end) in enumerate(chunks)}
        if state_path.exists() and part_path.exists():
            try:
                saved = json.loads(state_path.read_text(encoding="utf-8"))
                # A matching URL and length alone cannot prove a partial file
                # belongs to the current server object. Reusing it without an
                # ETag or Last-Modified can silently stitch two same-size
                # versions together after a CDN update.
                strong_etag = _strong_etag(metadata["etag"])
                has_validator = bool(strong_etag or metadata["last_modified"])
                saved_identity = str(saved.get("resource_key") or "")
                if not saved_identity:
                    # Version 1/2 checkpoints stored the URL. Read them once
                    # for compatibility; every subsequent save upgrades to a
                    # signature-free resource identity.
                    saved_identity = _resume_resource_identity(saved.get("url", ""))
                same_resource = saved_identity == _resume_resource_identity(task.url)
                validator_matches = (
                    _strong_etag(saved.get("etag", "")) == strong_etag
                    if strong_etag
                    else saved.get("last_modified", "") == metadata["last_modified"]
                )
                if (
                    has_validator
                    and same_resource
                    and validator_matches
                    and saved.get("total") == total
                ):
                    if int(saved.get("version", 1) or 1) >= 2:
                        for entry in saved.get("ranges", []):
                            try:
                                index = int(entry["index"])
                                start, end = chunks[index]
                                if int(entry["from"]) != start or int(entry["to"]) != end:
                                    continue
                                current = min(end + 1, max(start, int(entry["current"])))
                                range_current[index] = current
                                if current > end:
                                    completed.add(index)
                            except (IndexError, KeyError, TypeError, ValueError):
                                continue
                    else:
                        completed = {int(value) for value in saved.get("completed", []) if int(value) < len(chunks)}
                        for index in completed:
                            range_current[index] = chunks[index][1] + 1
            except (OSError, ValueError, TypeError):
                completed = set()
        if not part_path.exists() or part_path.stat().st_size != total:
            completed.clear()
            range_current = {index: start for index, (start, _end) in enumerate(chunks)}
            with part_path.open("wb") as output:
                output.truncate(total)
        self._written_intervals = {
            chunks[index][0]: current
            for index, current in range_current.items()
            if current > chunks[index][0]
        }
        task.progress.total_segments = len(chunks)
        task.progress.total_bytes = total
        self._completed_chunks = completed
        self._total_size = total
        task.engine_state["stream_path"] = str(part_path)
        task.engine_state["chunk_size"] = chunk_size
        task.engine_state["total_size"] = total
        task.progress.completed_segments = len(completed)
        task.progress.downloaded_bytes = sum(
            range_current[index] - chunks[index][0] for index in range(len(chunks))
        )
        # Enough workers to also split in-flight chunk tails (end-game), so
        # even a single-chunk file can use several connections.
        task.progress.max_workers = max(1, min(
            task.concurrency,
            max(len(chunks), total // (4 * 1024 * 1024) + 1),
        ))
        task.progress.connection_status = "running"
        set_connection_parts(
            task,
            build_connection_parts(
                total=total,
                chunks=chunks,
                range_current=range_current,
                completed=completed,
            ),
            total=total,
        )
        self._set_stage("downloading", f"正在分段下载，并发={task.progress.max_workers}")
        queue: asyncio.PriorityQueue[tuple[int, int]] = asyncio.PriorityQueue()
        self._priority_queue = queue
        pending = [index for index in range(len(chunks)) if index not in completed]
        if self._priority_chunk in pending:
            pending.remove(self._priority_chunk)
            pending.insert(0, self._priority_chunk)
        for order, index in enumerate(pending):
            queue.put_nowait((order, index))
        state_lock = asyncio.Lock()
        self._retry_window = SharedRetryWindow()
        retry_window = self._retry_window
        window = _SpeedWindow()
        # Bytes from finished chunks (including a resumed session's) are
        # "committed"; in-flight chunk bytes are tracked separately so a
        # failed chunk retracts cleanly and resumed bytes never inflate the
        # displayed transfer rate.
        committed_bytes = task.progress.downloaded_bytes
        partials: dict[tuple[int, int], int] = {}
        # Finished parts of a still-incomplete chunk are not resumable, so
        # they stay out of committed_bytes until the whole chunk lands.
        finished_parts: dict[int, int] = {}
        finished_intervals: dict[int, dict[int, int]] = {}
        last_publish = 0.0
        last_saved_bytes = task.progress.downloaded_bytes

        parts_open: dict[int, int] = {}

        def remember_connection_parts() -> None:
            parts = build_connection_parts(
                total=total,
                chunks=chunks,
                range_current=range_current,
                completed=completed,
                partials=partials,
                finished_intervals=finished_intervals,
            )
            set_connection_parts(task, parts, total=total)
            live = sum(max(0, int(parts_open.get(index, 0) or 0)) for index in parts_open)
            task.progress.active_workers = live
            task.progress.active_slots = live

        def refresh_progress(publish: bool = False) -> None:
            nonlocal last_publish
            task.progress.downloaded_bytes = (
                committed_bytes + sum(finished_parts.values()) + sum(partials.values())
            )
            self._apply_speed(window)
            now = time.monotonic()
            if publish or now - last_publish >= 0.5:
                last_publish = now
                remember_connection_parts()
                self._publish()

        def snapshot_currents() -> dict[int, int]:
            snapshot = dict(range_current)
            for index, (chunk_start, chunk_end) in enumerate(chunks):
                current = snapshot[index]
                while current <= chunk_end:
                    advanced = current
                    for start, end_exclusive in finished_intervals.get(index, {}).items():
                        if start <= current < end_exclusive:
                            advanced = max(advanced, end_exclusive)
                    for (part_index, start), received in partials.items():
                        end_exclusive = start + received
                        if part_index == index and start <= current < end_exclusive:
                            advanced = max(advanced, end_exclusive)
                    if advanced == current:
                        break
                    current = min(chunk_end + 1, advanced)
                snapshot[index] = current
            return snapshot

        async def save_state() -> int:
            nonlocal last_saved_bytes
            snapshot = snapshot_currents()
            payload = {
                "version": 3,
                "resource_key": _resume_resource_identity(task.url),
                "total": total,
                "etag": metadata["etag"],
                "last_modified": metadata["last_modified"],
                "ranges": [
                    {
                        "index": index,
                        "from": start,
                        "to": end,
                        "current": snapshot[index],
                    }
                    for index, (start, end) in enumerate(chunks)
                ],
            }
            checkpoint_json = json.dumps(payload)

            def persist_checkpoint() -> None:
                # The state file must never claim bytes that are only in the
                # OS/Python buffers. Flush payload first, then atomically
                # replace the checkpoint. Keep the Windows durability barrier
                # off the API event loop because it can block during scanning.
                with part_path.open("r+b", buffering=0) as durable_file:
                    os.fsync(durable_file.fileno())
                atomic_write_text(state_path, checkpoint_json)

            await asyncio.to_thread(persist_checkpoint)
            last_saved_bytes = sum(
                snapshot[index] - chunks[index][0] for index in range(len(chunks))
            )
            return last_saved_bytes

        # IDM-style end-game: when the queue drains, idle workers split the
        # remaining half of the largest in-flight part instead of exiting,
        # including tails created by an earlier split. A chunk is only
        # recorded as completed (resumable) when every part of it has
        # finished, so the on-disk state format is unchanged.
        part_stop: dict[tuple[int, int], int] = {}
        parts_open.clear()

        async def finish_part(index: int, part_key: tuple[int, int], size: int) -> None:
            nonlocal committed_bytes
            async with state_lock:
                finished_parts[index] = finished_parts.get(index, 0) + size
                partials.pop(part_key, None)
                finished_intervals.setdefault(index, {})[part_key[1]] = part_key[1] + size
                part_stop.pop(part_key, None)
                current = range_current[index]
                while True:
                    advanced = current
                    for start, end_exclusive in finished_intervals.get(index, {}).items():
                        if start <= current < end_exclusive:
                            advanced = max(advanced, end_exclusive)
                    if advanced == current:
                        break
                    current = min(chunks[index][1] + 1, advanced)
                range_current[index] = current
                parts_open[index] -= 1
                if parts_open[index] <= 0:
                    committed_bytes += finished_parts.pop(index, 0)
                    completed.add(index)
                    self._completed_chunks.add(index)
                    self._claimed_chunks.discard(index)
                    task.engine_state["completed_chunks"] = sorted(completed)
                    task.progress.completed_segments = len(completed)
                    await save_state()
                refresh_progress(publish=True)

        async def fetch_range(
            index: int,
            start: int,
            end: int,
            dynamic_stop: bool,
            *,
            playback_only: bool = False,
        ) -> bool:
            """Download one byte range. Returns False on a pause/cancel exit.

            With dynamic_stop the range belongs to a primary chunk whose end
            may shrink while streaming (an idle worker claimed its tail); the
            stream then finishes early at the shrunk boundary.
            """
            part_key = (index, start)
            last_error: Exception | None = None
            received = 0 if playback_only else int(partials.get(part_key, 0))
            source = self._pick_source(require_ranges=True)
            strong_etag = _strong_etag(source.etag or metadata.get("etag", ""))
            validator = strong_etag
            validator = validator or str(source.last_modified or metadata.get("last_modified", "") or "")
            for attempt in range(1, MAX_RETRIES + 1):
                if self._is_canceled():
                    raise asyncio.CancelledError
                if not await retry_window.wait(lambda: self._is_canceled() or self._is_pausing()):
                    return False
                self._clear_rate_limit_notice()
                url_generation = self._url_generation
                try:
                    while True:
                        target_end = part_stop.get(part_key, end) if dynamic_stop else end
                        request_start = start + received
                        if request_start > target_end:
                            break
                        request_headers = {
                            **headers,
                            "Range": f"bytes={request_start}-{target_end}",
                        }
                        if validator:
                            request_headers["If-Range"] = validator
                        async with client.stream(
                            "GET",
                            source.final_url or self._download_url,
                            headers=request_headers,
                        ) as response:
                            # 200 after a Range/If-Range request is a legitimate
                            # capability or object-version change, but its body
                            # starts at byte zero. Let the caller restart one
                            # sequential transfer instead of corrupting offsets.
                            if response.status_code == 200:
                                raise _HTTPRangeUnsupported(
                                    "服务器忽略了 Range 或远程文件版本已经变化"
                                )
                            response.raise_for_status()
                            content_range = _parse_content_range(
                                response.headers.get("content-range", "")
                            )
                            if response.status_code != 206 or content_range is None:
                                raise _HTTPRangeUnsupported(
                                    "Range 响应缺少有效 Content-Range"
                                )
                            response_start, response_end, response_total = content_range
                            if response_start != request_start:
                                raise _HTTPRangeValidationError(
                                    f"Range 起点不匹配，期望 {request_start}，实际 {response_start}"
                                )
                            if response_total is not None and response_total != total:
                                raise _HTTPRangeUnsupported(
                                    f"远程文件长度已从 {total} 变化为 {response_total}"
                                )
                            response_etag = response.headers.get("etag", "")
                            response_modified = response.headers.get("last-modified", "")
                            if strong_etag and response_etag and _strong_etag(response_etag) != strong_etag:
                                raise _HTTPRangeUnsupported("远程文件 ETag 已变化")
                            if (
                                not strong_etag
                                and metadata.get("last_modified")
                                and response_modified
                                and response_modified != metadata["last_modified"]
                            ):
                                raise _HTTPRangeUnsupported("远程文件修改时间已变化")
                            if _response_decodes_content(response):
                                raise _HTTPRangeUnsupported(
                                    "服务器对 Range 响应启用了内容压缩"
                                )

                            # Some CDNs cap each 206 response to a smaller range;
                            # others return a wider suffix. Accept both as long
                            # as the starting offset and object length are safe.
                            allowed_end = min(response_end, target_end)
                            expected_this_response = allowed_end - request_start + 1
                            response_received = 0
                            with part_path.open("r+b", buffering=0) as output_file:
                                output_file.seek(request_start)
                                pending = bytearray()

                                def flush_pending() -> None:
                                    nonlocal pending, response_received, received
                                    if not pending:
                                        return
                                    size = len(pending)
                                    written = output_file.write(pending)
                                    if written != size:
                                        raise OSError(
                                            f"本地文件写入不完整，期望 {size} 字节，实际 {written} 字节"
                                        )
                                    pending.clear()
                                    response_received += size
                                    received += size
                                    self._written_intervals[start] = max(
                                        self._written_intervals.get(start, start),
                                        start + received,
                                    )
                                    if not playback_only:
                                        partials[part_key] = received
                                        window.add(size)
                                        refresh_progress()

                                try:
                                    async for content in response.aiter_bytes():
                                        accepted = response_received + len(pending)
                                        if request_start == 0 and accepted == 0:
                                            validate_download_response(
                                                task,
                                                content_type=response.headers.get("content-type", ""),
                                                content_length=total,
                                                preview=bytes(content[:65536]),
                                                final_url=str(response.url),
                                                server_filename=str(metadata.get("filename", "") or ""),
                                            )
                                        live_end = part_stop.get(part_key, end) if dynamic_stop else end
                                        live_allowed_end = min(response_end, live_end)
                                        needed = live_allowed_end - request_start + 1 - accepted
                                        if needed <= 0:
                                            break
                                        data = content[:needed]
                                        await throttle_bytes(len(data), task)
                                        pending.extend(data)
                                        if len(pending) >= RANGE_WRITE_BATCH:
                                            flush_pending()
                                        if response_received + len(pending) >= live_allowed_end - request_start + 1:
                                            break
                                except BaseException:
                                    flush_pending()
                                    raise
                                flush_pending()
                            if response_received <= 0:
                                raise httpx.RemoteProtocolError(
                                    "Range 响应未返回任何数据"
                                )
                            # If the body ended before the range declared in its
                            # own Content-Range, preserve what arrived and retry
                            # from the exact next byte.
                            final_target_end = part_stop.get(part_key, end) if dynamic_stop else end
                            expected_this_response = min(response_end, final_target_end) - request_start + 1
                            if response_received < expected_this_response:
                                raise httpx.RemoteProtocolError(
                                    "Range 响应在声明的结束位置前中断"
                                )
                            # A server-capped response ends before our requested
                            # target. Loop immediately from the next byte.
                            continue
                    expected = (part_stop.get(part_key, end) if dynamic_stop else end) - start + 1
                    if received < expected:
                        raise httpx.RemoteProtocolError(
                            f"Range 长度不足，期望 {expected}，实际 {received}"
                        )
                    if received > expected:
                        raise _HTTPRangeValidationError(
                            f"Range 长度超过安全边界，期望 {expected}，实际 {received}"
                        )
                    if not playback_only:
                        await finish_part(index, part_key, expected)
                    return True
                except Exception as exc:
                    last_error = exc
                    if isinstance(exc, _HTTPRangeUnsupported):
                        if self._has_other_sources(source, require_ranges=True):
                            self._disable_source(source, str(exc))
                            source = self._pick_source(require_ranges=True)
                            strong_etag = _strong_etag(source.etag or metadata.get("etag", ""))
                            validator = strong_etag or str(source.last_modified or metadata.get("last_modified", "") or "")
                            continue
                        break
                    if isinstance(exc, _HTTPRangeValidationError):
                        break
                    status = int(
                        getattr(getattr(exc, "response", None), "status_code", 0) or 0
                    )
                    if status in {401, 403, 404, 410} and self._has_other_sources(source):
                        self._disable_source(source, f"HTTP {status}")
                        source = self._pick_source(require_ranges=True)
                        strong_etag = _strong_etag(source.etag or metadata.get("etag", ""))
                        validator = strong_etag or str(source.last_modified or metadata.get("last_modified", "") or "")
                        continue
                    if (
                        status in {401, 403, 404, 410}
                        and attempt < MAX_RETRIES
                        and await self._refresh_download_url(
                            client, headers, generation=url_generation
                        )
                    ):
                        refreshed = next((item for item in self._sources if not item.disabled), None)
                        if refreshed is not None:
                            source = refreshed
                            self._download_url = refreshed.final_url
                        continue
                    if not should_retry_download_error(exc):
                        break
                    if attempt < MAX_RETRIES:
                        delay = retry_delay_seconds(exc, min(4, attempt))
                        if should_share_retry_window(exc):
                            remaining, extended = await retry_window.extend(delay)
                            if extended:
                                self._announce_rate_limit(remaining)
                        else:
                            await asyncio.sleep(delay)
            if last_error is not None:
                if not playback_only:
                    self._claimed_chunks.discard(index)
                raise last_error
            return True

        async def fetch_playback_range(start: int, end: int) -> None:
            """Fetch only the bytes a local player needs without changing resume progress."""

            first = max(0, start // chunk_size)
            last = min(len(chunks) - 1, max(start, end) // chunk_size)
            for index in range(first, last + 1):
                chunk_start, chunk_end = chunks[index]
                request_start = max(start, chunk_start)
                request_end = min(end, chunk_end)
                if request_start > request_end or self._range_is_available(request_start, request_end):
                    continue
                await fetch_range(
                    index,
                    request_start,
                    request_end,
                    dynamic_stop=False,
                    playback_only=True,
                )

        self._playback_fetcher = fetch_playback_range

        def pick_split() -> tuple[int, int, int, int] | None:
            return pick_endgame_split(
                live_parts=part_stop,
                partials=partials,
                completed=completed,
            )

        async def worker() -> None:
            while not queue.empty() and not self._is_canceled() and not self._is_pausing():
                try:
                    _, index = queue.get_nowait()
                except asyncio.QueueEmpty:
                    break
                if index in completed or index in self._claimed_chunks or index >= len(chunks):
                    queue.task_done()
                    continue
                self._claimed_chunks.add(index)
                _chunk_start, end = chunks[index]
                start = range_current[index]
                if start > end:
                    completed.add(index)
                    self._completed_chunks.add(index)
                    queue.task_done()
                    continue
                part_stop[(index, start)] = end
                parts_open[index] = 1
                try:
                    ok = await fetch_range(index, start, end, dynamic_stop=True)
                finally:
                    part_stop.pop((index, start), None)
                if not ok:
                    self._claimed_chunks.discard(index)
                    queue.task_done()
                    return
                queue.task_done()
            # End-game: assist the slowest remaining chunk instead of idling.
            while not self._is_canceled() and not self._is_pausing():
                target = pick_split()
                if target is None:
                    return
                index, parent_start, split_start, split_end = target
                part_stop[(index, parent_start)] = split_start - 1
                part_stop[(index, split_start)] = split_end
                parts_open[index] += 1
                try:
                    if not await fetch_range(index, split_start, split_end, dynamic_stop=True):
                        return
                finally:
                    part_stop.pop((index, split_start), None)

        async def checkpoint_loop() -> None:
            while True:
                await asyncio.sleep(5.0)
                async with state_lock:
                    try:
                        await save_state()
                    except FileNotFoundError:
                        # Task deletion waits for the parent downloader, but a
                        # canceled parent used to leave this child behind. A
                        # missing payload is terminal for this checkpoint.
                        return

        checkpoint = asyncio.create_task(checkpoint_loop())
        workers = [asyncio.create_task(worker()) for _ in range(task.progress.max_workers)]
        try:
            results = await asyncio.gather(*workers, return_exceptions=True)
        finally:
            # Always reap children before the task manager is allowed to
            # remove the work directory. This also runs when app shutdown or
            # Delete cancels the parent at the gather() await above.
            for child in workers:
                if not child.done():
                    child.cancel()
            if workers:
                await asyncio.gather(*workers, return_exceptions=True)
            checkpoint.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await checkpoint
            self._playback_fetcher = None
            playback_fetches = [
                item for item in self._playback_fetch_tasks.values() if not item.done()
            ]
            for item in playback_fetches:
                item.cancel()
            if playback_fetches:
                await asyncio.gather(*playback_fetches, return_exceptions=True)
            self._playback_fetch_tasks.clear()
            if part_path.exists():
                with contextlib.suppress(FileNotFoundError):
                    async with state_lock:
                        await save_state()
        # Only report bytes covered by the durable checkpoint. An interrupted
        # request therefore resumes at the exact saved byte instead of the
        # start of its multi-megabyte chunk.
        partials.clear()
        finished_parts.clear()
        task.progress.downloaded_bytes = last_saved_bytes
        error = next(
            (result for result in results if isinstance(result, _HTTPRangeUnsupported)),
            None,
        ) or next((result for result in results if isinstance(result, Exception)), None)
        if error:
            raise error
        # Every worker has drained and the checkpoint is durable. Publish an
        # explicit terminal transfer sample before the potentially slow
        # cross-volume/antivirus-safe rename so the UI cannot remain at 99.x%
        # while the download is already complete on disk.
        if len(completed) == len(chunks) and part_path.exists() and part_path.stat().st_size == total:
            task.progress.downloaded_bytes = total
            task.progress.completed_segments = len(chunks)
            task.progress.active_workers = 0
            task.progress.active_slots = 0
            if total:
                set_connection_parts(
                    task,
                    [{"start": 0, "end": total - 1, "done": total, "state": "done"}],
                    total=total,
                )
            self._apply_speed(window)
            self._publish()
        self._priority_queue = None
