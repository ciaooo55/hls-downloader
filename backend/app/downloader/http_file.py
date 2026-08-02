from __future__ import annotations

import asyncio
import contextlib
import json
import mimetypes
import os
import re
import shutil
import time
from collections import deque
from datetime import datetime
from pathlib import Path
from urllib.parse import parse_qsl, urlencode, unquote, unquote_to_bytes, urlparse, urlsplit, urlunsplit

import httpx

from ..config import settings
from ..checksum import verify_task_checksum
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
    diagnose_download_error,
    format_download_error,
    retry_delay_seconds,
    should_retry_download_error,
    should_share_retry_window,
)
from .throttle import throttle_bytes
from .response_validation import validate_download_response


MAX_RETRIES = 5
PROBE_RESPONSE_TIMEOUT = 15.0
# The Range request and plain-GET fallback each have their own deadline. Keep
# an outer deadline too: a broken proxy/TLS close must never leave a task in
# "正在读取文件信息" forever.
PROBE_TOTAL_TIMEOUT = 35.0
PROBE_MAX_ATTEMPTS = 3
_CONTENT_RANGE_RE = re.compile(
    r"^\s*(?:bytes\s+)?(?P<start>\d+)\s*-\s*(?P<end>\d+)\s*/\s*(?P<total>\d+|\*)\s*$",
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
    path.parent.mkdir(parents=True, exist_ok=True)
    for index in range(10000):
        candidate = path if index == 0 else path.with_name(f"{path.stem}_{index}{path.suffix}")
        try:
            candidate.open("xb").close()
            return candidate
        except FileExistsError:
            continue
    raise RuntimeError(f"无法为输出文件分配唯一名称: {path.name}")


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
        self._playback_fetcher = None
        self._playback_fetch_tasks: dict[tuple[int, int], asyncio.Task] = {}
        self._chunk_size = max(1, int(settings.http_chunk_size_mb)) * 1024 * 1024
        self._part_path: Path | None = None
        self._total_size = 0
        self._sequential = False
        self._retry_window = SharedRetryWindow()
        self._last_rate_limit_notice = 0.0

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

    async def _probe(self, client: httpx.AsyncClient, headers: dict[str, str]) -> dict:
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
                    self.task.url,
                    headers={**headers, "Range": "bytes=0-255"},
                )
                ranged = await asyncio.wait_for(
                    client.send(request, stream=True),
                    timeout=PROBE_RESPONSE_TIMEOUT,
                )
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
                    request = client.build_request("GET", self.task.url, headers=headers)
                    fallback = await asyncio.wait_for(
                        client.send(request, stream=True),
                        timeout=PROBE_RESPONSE_TIMEOUT,
                    )
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
            total = int(content_range[2]) if range_supported and content_range else int(
                response.headers.get("content-length", 0)
                or (fallback.headers.get("content-length", 0) if fallback else 0)
                or 0
            )
            if encoded:
                # aiter_bytes yields the decoded entity, so the encoded wire
                # length cannot be used for allocation or final validation.
                total = 0
                range_supported = False
            filename = _content_disposition_filename(response.headers.get("content-disposition", ""))
            if not filename and fallback is not None:
                filename = _content_disposition_filename(fallback.headers.get("content-disposition", ""))
            preview = b""
            async for chunk in response.aiter_bytes():
                preview = bytes(chunk[:65536])
                break
            content_type = (response.headers.get("content-type", "") or (fallback.headers.get("content-type", "") if fallback else "")).split(";", 1)[0]
            validate_download_response(
                self.task,
                content_type=content_type,
                content_length=total,
                preview=preview,
                final_url=str(response.url),
                server_filename=filename,
            )
            return {
                "total": total,
                "ranges": range_supported,
                "etag": response.headers.get("etag", "") or (fallback.headers.get("etag", "") if fallback else ""),
                "last_modified": response.headers.get("last-modified", "") or (fallback.headers.get("last-modified", "") if fallback else ""),
                "content_type": content_type,
                "filename": filename,
                "final_url": str(response.url),
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
    ) -> dict:
        """Probe through transient CDN failures without hiding permanent errors."""
        last_error: Exception | None = None
        for attempt in range(1, PROBE_MAX_ATTEMPTS + 1):
            if self._is_canceled() or self._is_pausing():
                raise asyncio.CancelledError
            try:
                return await asyncio.wait_for(
                    self._probe(client, headers),
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
        task.progress.downloaded_bytes = 0
        task.progress.total_bytes = 0
        task.progress.progress_percent = 0.0
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
            limits = httpx.Limits(max_connections=max(2, task.concurrency + 2))
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
                    metadata = await self._probe_with_retry(client, headers)
                    total = int(metadata["total"])
                    self._total_size = total
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

                    if total <= 0 or not metadata["ranges"]:
                        self._sequential = True
                        await self._download_sequential(client, headers, part_path)
                    else:
                        try:
                            await self._download_ranges(client, headers, part_path, state_path, metadata)
                        except _HTTPRangeUnsupported:
                            # A CDN can advertise 206 during probing and later
                            # ignore Range or fail If-Range after an object
                            # rotation. Never stitch its full 200 response into
                            # sparse offsets: discard the range checkpoint and
                            # restart one verified sequential transfer.
                            self._sequential = True
                            self._completed_chunks.clear()
                            self._claimed_chunks.clear()
                            state_path.unlink(missing_ok=True)
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
            try:
                task.progress.downloaded_bytes = 0
                task.progress.progress_percent = 0.0
                async with client.stream("GET", task.url, headers=headers) as response:
                    response.raise_for_status()
                    content_type = response.headers.get("content-type", "").split(";", 1)[0]
                    task.mime_type = task.mime_type or content_type
                    reported_total = int(response.headers.get("content-length", 0) or 0)
                    if _response_decodes_content(response):
                        reported_total = 0
                    task.progress.total_bytes = reported_total
                    self._total_size = reported_total
                    task.engine_state["total_size"] = reported_total
                    with part_path.open("wb") as output:
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
                if not should_retry_download_error(exc) or attempt >= MAX_RETRIES:
                    break
                delay = retry_delay_seconds(exc, min(4, attempt))
                self._set_stage(
                    "downloading",
                    f"单连接传输中断，{delay:g} 秒后从头自动重试（{attempt}/{MAX_RETRIES - 1}）",
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

        def refresh_progress(publish: bool = False) -> None:
            nonlocal last_publish
            task.progress.downloaded_bytes = (
                committed_bytes + sum(finished_parts.values()) + sum(partials.values())
            )
            self._apply_speed(window)
            now = time.monotonic()
            if publish or now - last_publish >= 0.5:
                last_publish = now
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
        # remaining half of the largest in-flight chunk instead of exiting,
        # so the tail of a large file is never limited to one connection.
        # A chunk is only recorded as completed (resumable) when every part
        # of it has finished, so the on-disk state format is unchanged.
        SPLIT_MIN_BYTES = 4 * 1024 * 1024
        stop_at: dict[int, int] = {}
        parts_open: dict[int, int] = {}
        # Only a primary part that is still streaming may be split: once it
        # finishes, its bytes are committed and re-splitting that range would
        # re-download and double-count them.
        splittable: set[int] = set()
        primary_start: dict[int, int] = {}

        async def finish_part(index: int, part_key: tuple[int, int], size: int) -> None:
            nonlocal committed_bytes
            async with state_lock:
                finished_parts[index] = finished_parts.get(index, 0) + size
                partials.pop(part_key, None)
                finished_intervals.setdefault(index, {})[part_key[1]] = part_key[1] + size
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
            strong_etag = _strong_etag(metadata.get("etag", ""))
            validator = strong_etag
            validator = validator or str(metadata.get("last_modified", "") or "")
            for attempt in range(1, MAX_RETRIES + 1):
                if self._is_canceled():
                    raise asyncio.CancelledError
                if not await retry_window.wait(lambda: self._is_canceled() or self._is_pausing()):
                    return False
                self._clear_rate_limit_notice()
                try:
                    while True:
                        target_end = stop_at[index] if dynamic_stop else end
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
                            task.url,
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
                                async for content in response.aiter_bytes():
                                    if request_start == 0 and response_received == 0:
                                        validate_download_response(
                                            task,
                                            content_type=response.headers.get("content-type", ""),
                                            content_length=total,
                                            preview=bytes(content[:65536]),
                                            final_url=str(response.url),
                                            server_filename=str(metadata.get("filename", "") or ""),
                                        )
                                    live_end = stop_at[index] if dynamic_stop else end
                                    live_allowed_end = min(response_end, live_end)
                                    needed = live_allowed_end - request_start + 1 - response_received
                                    if needed <= 0:
                                        break
                                    data = content[:needed]
                                    await throttle_bytes(len(data), task)
                                    written = output_file.write(data)
                                    if written != len(data):
                                        raise OSError(
                                            f"本地文件写入不完整，期望 {len(data)} 字节，实际 {written} 字节"
                                        )
                                    response_received += len(data)
                                    received += len(data)
                                    self._written_intervals[start] = max(
                                        self._written_intervals.get(start, start),
                                        start + received,
                                    )
                                    if not playback_only:
                                        partials[part_key] = received
                                        window.add(len(data))
                                        refresh_progress()
                                    if response_received >= live_allowed_end - request_start + 1:
                                        break
                            if response_received <= 0:
                                raise httpx.RemoteProtocolError(
                                    "Range 响应未返回任何数据"
                                )
                            # If the body ended before the range declared in its
                            # own Content-Range, preserve what arrived and retry
                            # from the exact next byte.
                            final_target_end = stop_at[index] if dynamic_stop else end
                            expected_this_response = min(response_end, final_target_end) - request_start + 1
                            if response_received < expected_this_response:
                                raise httpx.RemoteProtocolError(
                                    "Range 响应在声明的结束位置前中断"
                                )
                            # A server-capped response ends before our requested
                            # target. Loop immediately from the next byte.
                            continue
                    expected = (stop_at[index] if dynamic_stop else end) - start + 1
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
                        break
                    if isinstance(exc, _HTTPRangeValidationError):
                        break
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

        def pick_split() -> tuple[int, int, int] | None:
            best: tuple[int, int, int] | None = None
            best_remaining = SPLIT_MIN_BYTES - 1
            for index in list(splittable):
                if index not in stop_at or index in completed:
                    continue
                chunk_start = primary_start.get(index, chunks[index][0])
                received = partials.get((index, chunk_start), 0)
                remaining = stop_at[index] - (chunk_start + received) + 1
                if remaining > best_remaining:
                    split_start = chunk_start + received + remaining // 2
                    best = (index, split_start, stop_at[index])
                    best_remaining = remaining
            return best

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
                stop_at[index] = end
                parts_open[index] = 1
                primary_start[index] = start
                splittable.add(index)
                try:
                    ok = await fetch_range(index, start, end, dynamic_stop=True)
                finally:
                    splittable.discard(index)
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
                index, split_start, split_end = target
                stop_at[index] = split_start - 1
                parts_open[index] += 1
                if not await fetch_range(index, split_start, split_end, dynamic_stop=False):
                    return

        async def checkpoint_loop() -> None:
            while True:
                await asyncio.sleep(1.0)
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
        self._priority_queue = None
