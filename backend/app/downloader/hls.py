import asyncio
import contextlib
import hashlib
import json
import os
import re
import shutil
from collections.abc import Awaitable, Callable
from dataclasses import replace
from datetime import datetime
from pathlib import Path
from typing import Any
from urllib.parse import parse_qsl, urlencode, urljoin, urlsplit, urlunsplit

import httpx
try:
    from curl_cffi.requests import AsyncSession as CurlAsyncSession
except ImportError:
    CurlAsyncSession = None
from cryptography.hazmat.primitives import padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

from ..config import settings
from ..checksum import verify_task_checksum
from ..models import Task, TaskProgress, TaskStatus
from ..utils import (
    atomic_write_text,
    canonical_hls_url,
    durable_replace,
    read_jsonl_prefix,
    sanitize_filename,
    stable_request_key,
    truncate_durable,
)
from ..naming import is_generic_media_name, suggest_manifest_name
from ..request_context import build_task_headers
from ..network_proxy import (
    curl_proxy,
    ensure_public_destination,
    ensure_url_allowed,
    network_budget,
    policy_httpx_client,
)
from .http_file import _content_disposition_filename
from .dash import DashDownloader
from .merge import merge_segments, mux_media_tracks
from .errors import (
    SharedRetryWindow,
    as_download_error,
    diagnose_download_error,
    format_download_error,
    retry_delay_seconds,
    should_retry_download_error,
    should_share_retry_window,
)
from .throttle import throttle_bytes
from .engine import task_output_dir, task_work_dir
from .parser import UnsupportedPlaylistError, filter_ad_segments, parse_m3u8
from .playback import playback_service, write_playback_plan
from .progress import ProgressTracker
from .subtitles import has_cues, merge_webvtt_segments, webvtt_to_srt


MAX_RETRIES = 5
MAX_PLAYLIST_DEPTH = 5
SEG_TIMEOUT = httpx.Timeout(connect=10, read=60, write=30, pool=30)
_CONTENT_RANGE_RE = re.compile(r"^bytes (\d+)-(\d+)/(\d+|\*)$", re.IGNORECASE)

# Live/event playlists keep growing, so recording polls the manifest instead
# of trusting a fixed segment list.  The stall window follows the HLS client
# guidance of several target durations: past it the origin has stopped
# publishing and the recording is finalized rather than left running forever.
LIVE_STATE_FILENAME = "live_state.json"
LIVE_STATE_JOURNAL_FILENAME = "live_state.journal"
LIVE_STATE_JOURNAL_MIN_COMPACT_BYTES = 4 * 1024 * 1024
LIVE_SUBTITLE_STATE_FILENAME = "live_subtitles.json"
LIVE_SUBTITLE_STATE_VERSION = 1
VOD_STATE_FILENAME = "vod_segments.json"
VOD_STATE_VERSION = 1
LIVE_STALL_MIN_SECONDS = 90.0
LIVE_STALL_TARGET_MULTIPLIER = 6.0
LIVE_BATCH_CONCURRENCY = 3
LIVE_MAX_POLL_SECONDS = 10.0


def _live_reload_delay(playlist: dict, received_new_segments: bool) -> float:
    """Choose an HLS reload cadence without losing LL-HLS partial windows."""
    try:
        part_target = max(0.0, float(playlist.get("part_target_duration") or 0))
    except (TypeError, ValueError):
        part_target = 0.0
    if part_target > 0:
        # PART-TARGET is the publication cadence. A one-second minimum (used
        # for ordinary HLS) can skip several parts on low-latency origins.
        return min(LIVE_MAX_POLL_SECONDS, max(0.2, part_target))
    try:
        target = max(1.0, float(playlist.get("target_duration") or 6.0))
    except (TypeError, ValueError):
        target = 6.0
    delay = target if received_new_segments else max(1.0, target / 2)
    return min(delay, LIVE_MAX_POLL_SECONDS)


def _blocking_reload_url(url: str, playlist: dict | None) -> str:
    """Build an LL-HLS blocking-reload URL from the last observed cursor.

    RFC 8216bis allows an origin advertising ``CAN-BLOCK-RELOAD=YES`` to hold
    the request until ``_HLS_msn``/``_HLS_part`` is newer than the current
    window.  A recorder that keeps polling the bare URL can repeatedly receive
    the same cached response and eventually lose the sliding PART window.
    Keep the signed URL intact, replace stale cursor parameters, and only send
    a cursor when the previous playlist exposes a concrete media sequence.
    """
    if not playlist or not playlist.get("can_block_reload"):
        return url
    segments = list(playlist.get("segments") or [])
    if not segments:
        return url
    last = segments[-1]
    try:
        media_sequence = int(last.get("media_sequence"))
    except (TypeError, ValueError):
        return url
    if media_sequence < 0:
        return url
    try:
        parsed = urlsplit(url)
        query = [
            (key, value)
            for key, value in parse_qsl(parsed.query, keep_blank_values=True)
            if key.lower() not in {"_hls_msn", "_hls_part", "_hls_skip"}
        ]
        query.append(("_HLS_msn", str(media_sequence)))
        part_index = last.get("part_index")
        if part_index is not None:
            query.append(("_HLS_part", str(max(0, int(part_index)))))
        return urlunsplit(parsed._replace(query=urlencode(query, doseq=True)))
    except (TypeError, ValueError):
        return url


class _BrowserHLSClient:
    def __init__(self, concurrency: int, url: str, deny_private_networks: bool = False) -> None:
        self._deny_private_networks = bool(deny_private_networks)
        self._session = CurlAsyncSession(
            max_clients=concurrency + 4,
            # Let curl-cffi emit the headers that match its TLS/browser
            # profile.  Replaying an extension's UA/sec-* fields here makes
            # that profile contradictory and is frequently rejected by
            # Cloudflare.
            default_headers=True,
            # HLS VOD downloads intentionally use independent HTTP/1.1
            # connections for the worker pool.  A number of video CDNs apply
            # their throughput limit to an individual HTTP/2 connection; if
            # all segment workers are multiplexed onto that one connection,
            # increasing task concurrency no longer increases total speed.
            # HTTP/1.1 keeps the configured worker count meaningful (the
            # same multi-connection strategy used by download managers),
            # while preserving the semantic access context (Referer, Origin,
            # Cookie and custom authorization headers).
            http_version="v1",
            timeout=(10, 60),
            allow_redirects=False,
        )

    async def __aenter__(self):
        await self._session.__aenter__()
        return self

    async def __aexit__(self, *args):
        return await self._session.__aexit__(*args)

    async def get(self, url: str, **kwargs):
        kwargs.setdefault("impersonate", _browser_impersonation())
        return await self._get_with_cloudflare_fallback(url, **kwargs)

    async def _get_with_cloudflare_fallback(self, url: str, **kwargs):
        response = await self._get_with_redirect_policy(url, **kwargs)
        fallback_headers = _without_stale_cloudflare_cookies(kwargs.get("headers"))
        if response.status_code != 403 or fallback_headers is None:
            return response

        # __cf_bm is short-lived telemetry, not a reusable login credential.
        # A stale one can cause a valid Referer/Origin/Cookie context to be
        # blocked.  Retry once with it removed; never loop on 403.
        await _close_response(response)
        retry_kwargs = dict(kwargs)
        retry_kwargs["headers"] = fallback_headers
        return await self._get_with_redirect_policy(url, **retry_kwargs)

    async def _get_with_redirect_policy(self, url: str, **kwargs):
        current = str(url)
        request_headers = dict(kwargs.get("headers") or {})
        for _hop in range(11):
            ensure_url_allowed(current)
            if self._deny_private_networks:
                await ensure_public_destination(current)
            request_kwargs = dict(kwargs)
            request_kwargs["headers"] = request_headers
            request_kwargs["allow_redirects"] = False
            proxy = curl_proxy(current)
            if proxy is None:
                request_kwargs.pop("proxy", None)
            else:
                request_kwargs["proxy"] = proxy
            slot_context = network_budget.slot(current)
            await slot_context.__aenter__()
            try:
                response = await self._session.get(current, **request_kwargs)
            except BaseException:
                await slot_context.__aexit__(None, None, None)
                raise
            network_budget.record_response(
                current,
                int(getattr(response, "status_code", 0) or 0),
                getattr(response, "headers", {}) or {},
            )
            if request_kwargs.get("stream"):
                setattr(response, "_hls_budget_context", slot_context)
            else:
                await slot_context.__aexit__(None, None, None)
            final_url = str(getattr(response, "url", "") or current)
            ensure_url_allowed(final_url)
            response_headers = getattr(response, "headers", {}) or {}
            location = str(response_headers.get("location", "") or "")
            if response.status_code not in {301, 302, 303, 307, 308} or not location:
                return response
            next_url = urljoin(final_url, location)
            ensure_url_allowed(next_url)
            if _url_authority(current) != _url_authority(next_url):
                request_headers = {
                    name: value
                    for name, value in request_headers.items()
                    if name.lower() not in {"authorization", "cookie", "proxy-authorization"}
                }
            await _close_response(response)
            current = next_url
        raise RuntimeError("HLS 请求重定向次数超过 10 次")

    async def download_to_file(
        self,
        url: str,
        destination: Path,
        headers: dict[str, str],
        cancel_check,
        task=None,
    ) -> tuple[Any, int]:
        written = 0
        response = await self._get_with_cloudflare_fallback(
            url,
            headers=headers,
            stream=True,
            impersonate=_browser_impersonation(),
        )
        try:
            # Do not write an HTML 403 page into the temporary media segment.
            # The caller validates and raises the real HTTP error below.
            if response.status_code >= 400:
                return response, written
            with destination.open("wb") as output:
                # curl-cffi defaults to small chunks.  Larger buffered writes
                # reduce Python/async scheduling overhead substantially when a
                # playlist contains thousands of short media segments, while
                # still keeping pause/cancel responsiveness below one chunk.
                # Keep a no-argument fallback for compatible response objects
                # whose iterator does not expose curl-cffi's chunk_size option.
                try:
                    content = response.aiter_content(chunk_size=256 * 1024)
                except TypeError:
                    content = response.aiter_content()
                async for chunk in content:
                    if cancel_check():
                        if response.quit_now:
                            response.quit_now.set()
                        raise asyncio.CancelledError
                    await throttle_bytes(len(chunk), task)
                    output.write(chunk)
                    written += len(chunk)
        finally:
            if response.astream_task and not response.astream_task.done():
                if response.quit_now:
                    response.quit_now.set()
            await _close_response(response)
        return response, written


def _browser_impersonation() -> str:
    """Use one supported curl-cffi profile, never a captured browser version."""
    return "chrome"


def _url_authority(url: str) -> tuple[str, str, int | None]:
    try:
        parsed = urlsplit(str(url or ""))
        return parsed.scheme.lower(), (parsed.hostname or "").lower(), parsed.port
    except (TypeError, ValueError):
        return "", "", None


def _without_stale_cloudflare_cookies(
    headers: dict[str, str] | None,
) -> dict[str, str] | None:
    """Return a one-shot 403 fallback header set without disposable CF cookies."""
    result = dict(headers or {})
    cookie_name = next((name for name in result if name.lower() == "cookie"), None)
    if not cookie_name:
        return None
    transient = {"__cf_bm", "__cflb"}
    values = []
    changed = False
    for item in str(result[cookie_name]).split(";"):
        name = item.split("=", 1)[0].strip().lower()
        if name in transient:
            changed = True
            continue
        if item.strip():
            values.append(item.strip())
    if not changed:
        return None
    if values:
        result[cookie_name] = "; ".join(values)
    else:
        result.pop(cookie_name, None)
    return result


async def _close_response(response: Any) -> None:
    close = getattr(response, "aclose", None)
    try:
        if close is not None:
            result = close()
            if hasattr(result, "__await__"):
                await result
    finally:
        slot_context = getattr(response, "_hls_budget_context", None)
        if slot_context is not None:
            setattr(response, "_hls_budget_context", None)
            await slot_context.__aexit__(None, None, None)


def _create_hls_client(
    concurrency: int,
    url: str = "",
    deny_private_networks: bool = False,
):
    if CurlAsyncSession is not None:
        return _BrowserHLSClient(concurrency, url, deny_private_networks)
    limits = httpx.Limits(
        max_connections=concurrency + 4,
        max_keepalive_connections=concurrency + 2,
    )
    return policy_httpx_client(
        timeout=SEG_TIMEOUT,
        follow_redirects=True,
        limits=limits,
        deny_private_networks=deny_private_networks,
    )


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


def _externally_cancelled() -> bool:
    """True when the running asyncio task itself has a pending cancel.

    A pause-abort raised inside a control request and a genuine
    task_handle.cancel() (e.g. app shutdown) both surface as
    CancelledError; only the latter increments the task's cancelling
    counter and must keep propagating, or shutdown would be blocked by a
    full merge of the recording.
    """
    current = asyncio.current_task()
    return bool(current is not None and current.cancelling())


def _format_clock(seconds: float) -> str:
    total = max(0, int(seconds))
    hours, remainder = divmod(total, 3600)
    minutes, secs = divmod(remainder, 60)
    if hours:
        return f"{hours}:{minutes:02d}:{secs:02d}"
    return f"{minutes:02d}:{secs:02d}"


def _decrypt_aes128_file(source: Path, destination: Path, key: bytes, iv: bytes) -> None:
    if len(key) != 16:
        raise ValueError(f"AES-128 密钥长度必须是 16 字节，实际为 {len(key)}")
    if len(iv) != 16:
        raise ValueError(f"AES-128 IV 长度必须是 16 字节，实际为 {len(iv)}")
    if source.stat().st_size % 16:
        raise ValueError("AES-128 加密分片长度不是 16 的倍数")

    temporary = destination.with_name(destination.name + ".decrypting")
    decryptor = Cipher(algorithms.AES(key), modes.CBC(iv)).decryptor()
    unpadder = padding.PKCS7(128).unpadder()
    try:
        with source.open("rb") as encrypted, temporary.open("wb") as output:
            while chunk := encrypted.read(1024 * 1024):
                output.write(unpadder.update(decryptor.update(chunk)))
            output.write(unpadder.update(decryptor.finalize()))
            output.write(unpadder.finalize())
        if temporary.stat().st_size == 0:
            raise ValueError("AES-128 解密结果为空")
        durable_replace(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)


class HLSDownloader:
    def __init__(self, task: Task, on_progress=None, on_log=None):
        self.task = task
        self.on_progress = on_progress or (lambda task: None)
        self.on_log = on_log or (lambda task_id, message: None)
        self.tracker = ProgressTracker()
        self._completed_count = 0
        self._failed_indexes: list[int] = []
        self._key_cache: dict[str, bytes] = {}
        self._last_segment_error: Exception | None = None
        self._playback_priority_index: int | None = task.playback_seek_index
        self._retry_window = SharedRetryWindow()
        self._last_rate_limit_notice = 0.0
        self._subtitle_tracks: list[dict] = []
        self._live_subtitle_stop: asyncio.Event | None = None
        self._live_subtitle_runner: asyncio.Task | None = None
        self._playback_refresh_error = ""
        self._vod_resume_enabled = False
        self._vod_resume_records: dict[str, dict[str, int | str]] = {}
        self._vod_resume_identities: dict[int, str] = {}
        self._vod_resume_lock = asyncio.Lock()
        self._live_checkpoint_records: dict[int, dict] | None = None
        self._live_checkpoint_duration = 0.0

    def _external_audio_task(self, url: str) -> Task:
        """Create a resumable sidecar recorder sharing only task controls."""
        output_dir = self._task_dir() / "external-audio"
        state = dict(self.task.engine_state)
        state["output_dir"] = str(output_dir)
        state.pop("live", None)
        state.pop("reserved_output_path", None)
        state.pop("output_is_file", None)
        return replace(
            self.task,
            id=f"{self.task.id}-audio",
            url=url,
            title="独立音轨",
            filename=f"{self.task.id}.audio.mp4",
            selected_video="",
            selected_audio="",
            status=TaskStatus.QUEUED,
            progress=TaskProgress(),
            error_message="",
            error_code="",
            error_stage="",
            error_url="",
            error_hint="",
            http_status=0,
            error_attempt=0,
            expected_checksum="",
            checksum_algorithm="",
            checksum_actual="",
            checksum_verified=None,
            output_path="",
            stage="queued",
            last_log="等待独立音轨录制",
            started_at="",
            finished_at="",
            updated_at="",
            cancel_event=self.task.cancel_event,
            pause_event=asyncio.Event(),
            task_handle=None,
            playback_seek_index=None,
            engine_state=state,
        )

    def _start_external_audio_recorder(self, url: str) -> tuple[Task, asyncio.Task]:
        audio_task = self._external_audio_task(url)
        downloader = HLSDownloader(
            audio_task,
            on_progress=lambda _task: None,
            on_log=lambda _task_id, message: self.on_log(
                self.task.id, f"[external_audio] {message}"
            ),
        )
        runner = asyncio.create_task(
            downloader.run(), name=f"hls-audio-{self.task.id}"
        )
        return audio_task, runner

    async def _finish_external_audio_recorder(
        self, audio_task: Task, runner: asyncio.Task
    ) -> Path | None:
        if audio_task.pause_event is not None:
            audio_task.pause_event.set()
        await runner
        output = Path(audio_task.output_path) if audio_task.output_path else None
        if audio_task.status is not TaskStatus.DONE or output is None or not output.is_file():
            detail = audio_task.error_message or audio_task.last_log or "独立音轨没有生成输出"
            # A separate audio rendition is optional from the user's point of
            # view. Keep the already merged video usable when that rendition
            # expires, is blocked, or disappears at a live poll boundary.
            self._log(f"[external_audio] 独立音轨不可用，保留无外挂音频的视频成品: {detail}")
            return None
        return output

    def request_seek(self, segment_index: int) -> None:
        if segment_index >= 0:
            self._playback_priority_index = int(segment_index)
            self.task.playback_seek_index = int(segment_index)

    def _headers(
        self,
        request_url: str = "",
        base_headers: dict[str, str] | None = None,
    ) -> dict[str, str]:
        return build_task_headers(
            self.task,
            # curl-cffi supplies the Accept header associated with its
            # impersonation profile; do not override it with a captured one.
            accept="",
            request_url=request_url,
            base_headers=base_headers,
        )

    def _task_dir(self) -> Path:
        return task_work_dir(self.task)

    def _seg_dir(self) -> Path:
        return self._task_dir() / "segments"

    def _output_path(self) -> Path:
        filename = sanitize_filename(self.task.filename or self.task.title or self.task.id)
        if not filename.lower().endswith(".mp4"):
            filename += ".mp4"
        return _reserve_output_path(task_output_dir(self.task) / filename)

    def _publish(self) -> None:
        self.on_progress(self.task)

    def _set_stage(self, stage: str, message: str) -> None:
        self.task.stage = stage
        self.task.last_log = message
        self._log(f"[{stage}] {message}")
        self._publish()

    def _log(self, message: str) -> None:
        self.on_log(self.task.id, message)

    def _is_canceled(self) -> bool:
        return bool(self.task.cancel_event and self.task.cancel_event.is_set())

    def _is_pausing(self) -> bool:
        return bool(self.task.pause_event and self.task.pause_event.is_set())

    def _should_propagate_cancellation(self) -> bool:
        """Keep only intentional task cancellation on asyncio's cancel path.

        curl_cffi can surface a transport reset as ``CancelledError`` even
        though neither the task nor its worker was cancelled.  Treating that
        as a task cancellation pauses a healthy download after a random
        segment.  A real task cancellation increments ``cancelling()``, while
        the two explicit user actions are represented by their task events.
        """
        return (
            self._is_canceled()
            or self._is_pausing()
            or _externally_cancelled()
        )

    @staticmethod
    def _unexpected_request_cancellation() -> httpx.RemoteProtocolError:
        """Normalize a transport's spurious cancellation into a retryable error."""
        return httpx.RemoteProtocolError("底层网络请求意外中断")

    def _announce_rate_limit(self, remaining: float) -> None:
        now = asyncio.get_running_loop().time()
        if now - self._last_rate_limit_notice < 0.5:
            return
        self._last_rate_limit_notice = now
        seconds = max(1, int(remaining + 0.999))
        self.task.progress.connection_status = "rate_limited"
        self._set_stage("downloading_segments", f"服务器限流，所有分片等待约 {seconds} 秒")

    def _clear_rate_limit_notice(self) -> None:
        if self.task.progress.connection_status != "rate_limited":
            return
        self.task.progress.connection_status = "running"
        self._set_stage("downloading_segments", "服务器限流结束，继续下载")

    def _clear_failure(self) -> None:
        self.task.error_message = ""
        self.task.error_code = ""
        self.task.error_stage = ""
        self.task.error_url = ""
        self.task.error_hint = ""
        self.task.http_status = 0
        self.task.error_attempt = 0

    def _record_failure(self, exc: BaseException, *, stage: str, url: str = "") -> None:
        details = diagnose_download_error(
            exc,
            stage=stage,
            url=url or self.task.url,
            task_context=self.task,
        )
        self.task.error_code = details.code
        self.task.error_stage = details.stage
        self.task.error_url = details.url
        self.task.error_hint = details.hint
        self.task.http_status = details.http_status
        self.task.error_attempt = details.attempt
        self.task.error_message = format_download_error(details)

    async def _cleanup_failed_temp(self, task_dir: Path) -> None:
        if settings.keep_temp_files or not task_dir.exists():
            return
        if self.task.engine_state.get("live"):
            # A live stream cannot be downloaded again: keep the captured
            # segments and live_state.json so retry can finalize them even
            # after a merge failure or a dead manifest.
            return
        if (
            (task_dir / VOD_STATE_FILENAME).is_file()
            and any(
                path.is_file() and path.stat().st_size > 0
                for path in (task_dir / "segments").glob("*.seg")
            )
        ):
            # Verified VOD slots are resumable and may represent gigabytes.
            # A retry should fetch only the failed pieces, not discard all
            # durable work merely because one segment exhausted its retries.
            return

        def cleanup() -> None:
            keep = {"download.log", "playlist.m3u8"}
            for child in task_dir.iterdir():
                if child.name in keep:
                    continue
                if child.is_dir():
                    shutil.rmtree(child, ignore_errors=True)
                else:
                    child.unlink(missing_ok=True)

        await asyncio.to_thread(
            playback_service.cleanup_if_inactive,
            self.task.id,
            cleanup,
        )

    async def _cleanup_task_dir(self, task_dir: Path) -> None:
        if settings.keep_temp_files or not task_dir.exists():
            return
        await asyncio.to_thread(
            playback_service.cleanup_if_inactive,
            self.task.id,
            lambda: shutil.rmtree(task_dir, ignore_errors=True),
        )

    async def _retry_control_request(
        self,
        operation: Callable[[], Awaitable[Any]],
        *,
        stage: str,
        url: str,
        label: str,
    ) -> Any:
        """Retry small HLS control resources without treating auth failures as transient.

        A playlist, AES key, or fMP4 init map is fetched before (or outside)
        the normal segment worker pool.  Those requests previously bypassed
        the segment retry logic, so one brief 429/503/timeout could discard an
        otherwise healthy video.  Keep the retry policy identical to segments,
        including Retry-After and the task-local cooldown, while preserving
        non-retryable 401/403/404 failures for actionable diagnostics.
        """
        last_error: Exception | None = None
        attempts_made = 0
        for attempt in range(MAX_RETRIES):
            attempts_made = attempt + 1
            if self._is_canceled() or self._is_pausing():
                raise asyncio.CancelledError
            if not await self._retry_window.wait(
                lambda: self._is_canceled() or self._is_pausing()
            ):
                raise asyncio.CancelledError
            try:
                value = await operation()
                if self.task.progress.connection_status == "rate_limited":
                    self.task.progress.connection_status = (
                        "running" if stage == "downloading_segments" else "connecting"
                    )
                    self._set_stage(stage, f"{label}限流结束，继续请求")
                return value
            except asyncio.CancelledError:
                if self._should_propagate_cancellation():
                    raise
                # curl_cffi may report a dropped TLS/socket stream as a
                # cancellation of its internal request task.  It is not a
                # pause of the download, so retry it just like a protocol
                # disconnect instead of bubbling it to run().
                last_error = self._unexpected_request_cancellation()
                self._log(
                    f"[{label}] 第 {attempt + 1}/{MAX_RETRIES} 次请求被网络层中断，正在重试"
                )
                if attempt >= MAX_RETRIES - 1:
                    break
                await asyncio.sleep(retry_delay_seconds(last_error, min(2**attempt, 10)))
            except Exception as exc:
                last_error = exc
                if not should_retry_download_error(exc):
                    break
                if attempt >= MAX_RETRIES - 1:
                    break
                delay = retry_delay_seconds(exc, min(2**attempt, 10))
                if should_share_retry_window(exc):
                    remaining, extended = await self._retry_window.extend(delay)
                    if extended:
                        seconds = max(1, int(remaining + 0.999))
                        self.task.progress.connection_status = "rate_limited"
                        self._set_stage(
                            stage,
                            f"服务器限流，{label}等待约 {seconds} 秒",
                        )
                else:
                    self._log(
                        f"[{label}] 第 {attempt + 1}/{MAX_RETRIES} 次失败: {exc}"
                    )
                    await asyncio.sleep(delay)
        if last_error is None:
            raise RuntimeError(f"{label}请求失败")
        raise as_download_error(
            last_error,
            stage=stage,
            url=url,
            attempt=attempts_made,
            task_context=self.task,
        ) from last_error

    async def _load_media_playlist(
        self,
        client: Any,
        url: str,
        headers: dict[str, str],
    ) -> dict:
        visited: set[str] = set()
        current_url = url
        manifest_title = ""
        response_filename = ""
        external_audio = False
        external_audio_url = ""
        audio_tracks: list[dict] = []
        subtitle_tracks: list[dict] = []
        for depth in range(MAX_PLAYLIST_DEPTH + 1):
            if current_url in visited:
                raise ValueError(f"主清单存在循环引用: {current_url}")
            visited.add(current_url)
            async def load_playlist():
                response = await client.get(
                    current_url,
                    headers=self._headers(current_url, headers),
                )
                response.raise_for_status()
                return response

            response = await self._retry_control_request(
                load_playlist,
                stage="parsing",
                url=current_url,
                label="HLS 清单",
            )
            final_url = str(getattr(response, "url", "") or current_url)
            # The chosen rendition is resolved inside the master so
            # EXT-X-MEDIA audio/subtitle detection is never bypassed.
            parsed = parse_m3u8(
                final_url,
                response.text,
                self.task.selected_video,
                self.task.selected_audio,
            )
            manifest_title = manifest_title or parsed.get("title", "")
            response_filename = response_filename or _content_disposition_filename(
                response.headers.get("content-disposition", "")
            )
            if parsed["type"] == "media":
                parsed = filter_ad_segments(
                    parsed,
                    enabled=bool(getattr(settings, "skip_ad_segments", True)),
                )
                skipped = int(parsed.get("ad_segments_skipped") or 0)
                if skipped:
                    self._log(f"[parsing] 已跳过 {skipped} 个 HLS 广告标记分片")
                parsed["content"] = response.text
                parsed["title"] = manifest_title
                parsed["response_filename"] = response_filename
                parsed["final_url"] = final_url
                parsed["external_audio"] = external_audio
                parsed["external_audio_url"] = external_audio_url
                parsed["audio_tracks"] = audio_tracks
                parsed["subtitle_tracks"] = subtitle_tracks
                return parsed
            if parsed.get("subtitle_tracks") and not subtitle_tracks:
                subtitle_tracks = list(parsed["subtitle_tracks"])
            if parsed.get("external_audio"):
                # The native segment engine intentionally keeps one media
                # timeline at a time.  A master with a separate audio
                # rendition is still fully downloadable through the bundled
                # adaptive compatibility engine, which selects and muxes the
                # matching best video/audio pair without dropping auth context.
                external_audio = True
                external_audio_url = str(parsed.get("external_audio_url") or "")
                audio_tracks = list(parsed.get("audio_tracks") or [])
            if parsed.get("external_subtitles"):
                if subtitle_tracks and getattr(settings, "download_subtitles", True):
                    self._log(
                        f"[parsing] 检测到 {len(subtitle_tracks)} 条外部字幕，"
                        "下载完成后将保存为独立字幕文件"
                    )
                else:
                    self._log("[parsing] 外部字幕轨道已忽略")
            if depth >= MAX_PLAYLIST_DEPTH:
                raise ValueError(f"主清单递归超过 {MAX_PLAYLIST_DEPTH} 层")
            current_url = parsed["url"]
        raise ValueError("无法解析媒体清单")

    async def run(self) -> None:
        task = self.task
        # webRequest observes LL-HLS blocking reloads with a moving cursor.
        # That poll position expires almost immediately and is not the stable
        # playlist address a recorder should persist or retry.
        task.url = canonical_hls_url(task.url)
        task_dir = self._task_dir()
        seg_dir = self._seg_dir()
        seg_dir.mkdir(parents=True, exist_ok=True)
        output: Path | None = None
        external_audio_task: Task | None = None
        external_audio_runner: asyncio.Task | None = None

        try:
            self._clear_failure()
            task.status = TaskStatus.DOWNLOADING_M3U8
            task.started_at = task.started_at or datetime.now().isoformat()
            task.progress.connection_status = "connecting"
            self._set_stage("downloading_m3u8", "正在获取 m3u8 清单")

            concurrency = min(64, max(1, int(task.concurrency or settings.default_concurrency or 12)))
            task.concurrency = concurrency
            headers = self._headers(task.url)
            async with _create_hls_client(
                concurrency,
                task.url,
                bool(task.engine_state.get("browser_originated")),
            ) as client:
                task.status = TaskStatus.PARSING
                self._set_stage("parsing", "正在解析 HLS 清单")
                saved_live_state = self._load_live_state()
                try:
                    parsed = await self._load_media_playlist(client, task.url, headers)
                except asyncio.CancelledError:
                    raise
                except Exception:
                    # A finished live stream often takes its manifest offline.
                    # The captured segments are the only copy that will ever
                    # exist, so finalize them instead of failing the task.
                    if not (saved_live_state and saved_live_state.get("segments")):
                        raise
                    self._log("[recording] 直播清单已不可用，直接合并已录制的内容")
                    parsed = None
                # A finished stream may replay as VOD, but a resumed recording
                # must stay in recording mode so earlier segments are kept.
                is_live = parsed is None or bool(parsed.get("is_live")) or saved_live_state is not None
                if parsed is not None:
                    self._subtitle_tracks = list(parsed.get("subtitle_tracks") or [])
                    if is_generic_media_name(task.filename):
                        task.filename = suggest_manifest_name(
                            parsed.get("final_url") or task.url,
                            filename=task.filename,
                            title=task.title,
                            source_page_url=task.source_page_url,
                            manifest_title=parsed.get("title", ""),
                            response_filename=parsed.get("response_filename", ""),
                            fallback=task.id,
                        )
                    if parsed.get("external_audio"):
                        if is_live:
                            audio_url = str(parsed.get("external_audio_url") or "")
                            if not audio_url:
                                raise UnsupportedPlaylistError("独立 HLS 音轨缺少可下载的 URI")
                            self._set_stage(
                                "parsing", "检测到直播独立音轨，正在同步录制视频与音频"
                            )
                            external_audio_task, external_audio_runner = (
                                self._start_external_audio_recorder(audio_url)
                            )
                        else:
                            self._set_stage("parsing", "检测到独立 HLS 音轨，正在使用兼容合并引擎")
                            await DashDownloader(
                                task,
                                on_progress=self.on_progress,
                                on_log=self.on_log,
                                source_label="HLS 独立音轨",
                            ).run()
                            return
                    (task_dir / "playlist.m3u8").write_text(parsed["content"], encoding="utf-8")

                if is_live:
                    task.engine_state["live"] = True
                    if self._subtitle_tracks and getattr(settings, "download_subtitles", True):
                        self._log(
                            f"[subtitles] 同步录制 {len(self._subtitle_tracks)} 条直播字幕轨道"
                        )
                        self._start_live_subtitle_recorder(headers)
                    if parsed is None:
                        assert saved_live_state is not None
                        recovered: list[dict] = []
                        total_duration = self._restore_live_segments(
                            saved_live_state, recovered
                        )
                        if not recovered:
                            raise RuntimeError("直播源已不可用，且没有可合并的已录制分片")
                        await asyncio.to_thread(self._compact_recorded, recovered)
                        await asyncio.to_thread(self._save_live_state, recovered, total_duration)
                        segments = recovered
                        task.progress.total_segments = len(segments)
                        task.progress.media_duration = total_duration
                        await asyncio.to_thread(write_playback_plan, task_dir, segments, total_duration)
                    else:
                        recorded = await self._record_live(
                            client, parsed, headers, saved_live_state
                        )
                        if recorded is None:
                            if external_audio_task and external_audio_task.pause_event:
                                external_audio_task.pause_event.set()
                            return
                        segments, total_duration = recorded
                        if external_audio_task and external_audio_task.pause_event:
                            external_audio_task.pause_event.set()
                    await self._stop_live_subtitle_recorder()
                else:
                    # A retried task whose stream has since ended downloads as
                    # plain VOD; drop the stale live marker so the UI stops
                    # presenting it as a recording.
                    task.engine_state.pop("live", None)
                    assert parsed is not None
                    segments = parsed["segments"]
                    if not segments:
                        raise ValueError("m3u8 中没有分片")
                    total_duration = float(parsed["total_duration"] or 0)

                    task.progress.total_segments = len(segments)
                    self._set_stage("parsing", f"解析完成，共 {len(segments)} 个分片")
                    await self._download_init_maps(client, segments, headers)
                    await asyncio.to_thread(self._prepare_vod_resume, segments)
                    await asyncio.to_thread(write_playback_plan, task_dir, segments, total_duration)
                    task.progress.media_duration = total_duration
                    self._refresh_playback_progress()

                    task.status = TaskStatus.DOWNLOADING_SEGMENTS
                    task.progress.max_workers = concurrency
                    task.progress.connection_status = "running"
                    self._set_stage(
                        "downloading_segments",
                        f"开始下载 {len(segments)} 个分片，并发={concurrency}",
                    )
                    completed = await self._download_segments(client, segments, headers, concurrency)
                    if not completed:
                        if self._is_canceled():
                            task.status = TaskStatus.CANCELED
                            self._set_stage("canceled", "已取消")
                        elif self._is_pausing():
                            task.status = TaskStatus.PAUSED
                            task.progress.connection_status = "idle"
                            self._set_stage("paused", "已暂停，可继续下载")
                        return

                    if self._failed_indexes:
                        if self._last_segment_error is not None:
                            raise self._last_segment_error
                        raise RuntimeError(
                            f"{len(self._failed_indexes)} 个分片下载失败，共 {len(segments)} 个"
                        )

            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                self._set_stage("canceled", "已取消")
                return

            task.status = TaskStatus.MERGING
            task.progress.connection_status = "idle"
            task.progress.post_percent = 0.0
            self._set_stage("merging", f"正在准备 {len(segments)} 个分片")
            output = self._output_path()
            task.engine_state["reserved_output_path"] = str(output)
            await merge_segments(
                seg_dir=seg_dir,
                output_path=output,
                segments=segments,
                ffmpeg_path=settings.ffmpeg_path,
                task=task,
                total_duration=total_duration,
                on_progress=self.on_progress,
                on_log=self.on_log,
            )
            if external_audio_task is not None and external_audio_runner is not None:
                audio_output = await self._finish_external_audio_recorder(
                    external_audio_task, external_audio_runner
                )
                external_audio_runner = None
                if audio_output is not None:
                    self._set_stage("remuxing", "正在合并直播视频与独立音轨")
                    await mux_media_tracks(
                        video_path=output,
                        audio_path=audio_output,
                        output_path=output,
                        ffmpeg_path=settings.ffmpeg_path,
                        task=task,
                        total_duration=total_duration,
                        on_progress=self.on_progress,
                        on_log=self.on_log,
                    )

            task.output_path = str(output)
            task.engine_state["output_is_file"] = True
            task.engine_state.pop("reserved_output_path", None)
            # Segment protocols can only estimate the network byte total while
            # downloading.  Once the merged output is durable, expose its exact
            # file size so a completed HLS task never keeps showing the old
            # segment estimate (or "unknown") in the task list.
            output_size = output.stat().st_size
            task.engine_state["stream_path"] = str(output)
            task.engine_state["total_size"] = output_size
            task.progress.downloaded_bytes = output_size
            task.progress.total_bytes = output_size
            if not await verify_task_checksum(task, output, on_progress=self.on_progress, on_log=self.on_log):
                return
            if task.engine_state.get("live"):
                await self._save_recorded_live_subtitles()
            else:
                # Sidecar subtitles are best-effort: a subtitle CDN failure
                # must never fail a fully merged, verified video.
                await self._download_subtitles(headers)
            task.status = TaskStatus.DONE
            task.finished_at = datetime.now().isoformat()
            task.progress.post_percent = 100.0
            task.progress.connection_status = "idle"
            size_mb = output_size / 1048576
            self._set_stage("done", f"完成: {output.name} ({size_mb:.1f} MB)")
            await self._cleanup_task_dir(task_dir)

        except asyncio.CancelledError:
            task.progress.connection_status = "idle"
            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                task.finished_at = datetime.now().isoformat()
                self._set_stage("canceled", "已取消")
                if output and output.exists() and output.stat().st_size == 0:
                    output.unlink(missing_ok=True)
                await self._cleanup_task_dir(task_dir)
            else:
                task.status = TaskStatus.PAUSED
                task.stage = "interrupted"
                task.last_log = "程序已关闭，分片已保留，可在下次启动后恢复"
                self._publish()
        except UnsupportedPlaylistError as exc:
            failure_stage = task.stage
            self._record_failure(exc, stage=failure_stage)
            task.status = TaskStatus.UNSUPPORTED
            task.finished_at = datetime.now().isoformat()
            task.progress.connection_status = "error"
            self._set_stage("unsupported", task.error_message)
            await self._cleanup_failed_temp(task_dir)
        except Exception as exc:
            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                task.finished_at = datetime.now().isoformat()
                self._set_stage("canceled", "已取消")
            else:
                failure_stage = task.stage
                self._record_failure(exc, stage=failure_stage)
                task.status = TaskStatus.FAILED
                task.finished_at = datetime.now().isoformat()
                task.progress.connection_status = "error"
                self._set_stage("failed", task.error_message)
            if output and output.exists() and output.stat().st_size == 0:
                output.unlink(missing_ok=True)
            if task.status is TaskStatus.FAILED:
                await self._cleanup_failed_temp(task_dir)
        finally:
            await self._stop_live_subtitle_recorder()
            if external_audio_runner is not None and not external_audio_runner.done():
                external_audio_runner.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await external_audio_runner
            task.progress.active_workers = 0
            task.progress.active_slots = 0
            task.progress.active_segment_indexes = []
            self._publish()

    def _live_subtitle_state_path(self) -> Path:
        return self._task_dir() / LIVE_SUBTITLE_STATE_FILENAME

    def _load_live_subtitle_state(self) -> dict:
        try:
            state = json.loads(self._live_subtitle_state_path().read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return {"version": LIVE_SUBTITLE_STATE_VERSION, "tracks": {}}
        if state.get("version") != LIVE_SUBTITLE_STATE_VERSION or not isinstance(
            state.get("tracks"), dict
        ):
            return {"version": LIVE_SUBTITLE_STATE_VERSION, "tracks": {}}
        return state

    def _save_live_subtitle_state(self, state: dict) -> None:
        atomic_write_text(
            self._live_subtitle_state_path(),
            json.dumps(state, ensure_ascii=False, sort_keys=True),
        )

    @staticmethod
    def _live_subtitle_track_key(track: dict) -> str:
        identity = stable_request_key(str(track.get("uri") or ""), ignore_host=True)
        if not identity:
            identity = f"{track.get('language') or ''}:{track.get('name') or ''}"
        return hashlib.sha256(identity.encode("utf-8")).hexdigest()[:20]

    def _start_live_subtitle_recorder(self, headers: dict[str, str]) -> None:
        if self._live_subtitle_runner and not self._live_subtitle_runner.done():
            return
        self._live_subtitle_stop = asyncio.Event()
        self._live_subtitle_runner = asyncio.create_task(
            self._record_live_subtitles(headers, self._live_subtitle_stop)
        )

    async def _stop_live_subtitle_recorder(self) -> None:
        runner = self._live_subtitle_runner
        stop = self._live_subtitle_stop
        if runner is None:
            return
        if stop is not None:
            stop.set()
        try:
            await asyncio.wait_for(asyncio.shield(runner), timeout=3)
        except TimeoutError:
            runner.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await runner
        except asyncio.CancelledError:
            runner.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await runner
            raise
        finally:
            self._live_subtitle_runner = None
            self._live_subtitle_stop = None

    async def _record_live_subtitles(
        self,
        headers: dict[str, str],
        stop: asyncio.Event,
    ) -> None:
        state = self._load_live_subtitle_state()
        failure_notice: set[str] = set()
        async with _create_hls_client(
            2,
            self.task.url,
            bool(self.task.engine_state.get("browser_originated")),
        ) as client:
            while not stop.is_set() and not self._is_canceled():
                for track in self._subtitle_tracks:
                    if stop.is_set() or self._is_canceled():
                        break
                    key = self._live_subtitle_track_key(track)
                    try:
                        changed = await self._capture_live_subtitle_track(
                            client, track, key, state, headers
                        )
                        if changed:
                            await asyncio.to_thread(self._save_live_subtitle_state, state)
                        failure_notice.discard(key)
                    except asyncio.CancelledError:
                        raise
                    except Exception as exc:
                        if key not in failure_notice:
                            failure_notice.add(key)
                            self._log(
                                f"[subtitles] 直播字幕 {track.get('language') or track.get('name') or key} "
                                f"暂时不可用，将继续重试: {exc}"
                            )
                try:
                    await asyncio.wait_for(stop.wait(), timeout=2.0)
                except TimeoutError:
                    pass

    async def _capture_live_subtitle_track(
        self,
        client: Any,
        track: dict,
        key: str,
        state: dict,
        headers: dict[str, str],
    ) -> bool:
        url = str(track.get("uri") or "")
        if not url:
            return False
        response = await client.get(url, headers=self._headers(url, headers))
        response.raise_for_status()
        final_url = str(getattr(response, "url", "") or url)
        track_state = state["tracks"].setdefault(
            key,
            {
                "name": str(track.get("name") or ""),
                "language": str(track.get("language") or ""),
                "forced": bool(track.get("forced")),
                "segments": [],
            },
        )
        segments = track_state.setdefault("segments", [])
        seen = {str(item.get("identity") or "") for item in segments if isinstance(item, dict)}
        cache_dir = self._task_dir() / "live-subtitles" / key
        cache_dir.mkdir(parents=True, exist_ok=True)
        changed = False

        candidates: list[tuple[str, dict | None, bytes | None]]
        if response.text.lstrip("﻿ \t\r\n").startswith("WEBVTT"):
            payload = bytes(response.content)
            identity = hashlib.sha256(payload).hexdigest()
            candidates = [(identity, None, payload)]
        else:
            parsed = parse_m3u8(final_url, response.text)
            if parsed["type"] != "media":
                raise RuntimeError("直播字幕清单不是媒体清单")
            candidates = [
                (self._vod_segment_identity(segment), segment, None)
                for segment in parsed["segments"]
                if segment.get("url")
            ]

        for identity, segment, direct_payload in candidates:
            if not identity or identity in seen:
                continue
            filename = f"{len(segments):08d}.vtt"
            destination = cache_dir / filename
            if direct_payload is not None:
                temporary = destination.with_name(destination.name + ".tmp")
                temporary.write_bytes(direct_payload)
                await asyncio.to_thread(durable_replace, temporary, destination)
            else:
                await self._download_subtitle_segment(
                    client, segment or {}, headers, destination
                )
            if destination.stat().st_size <= 0:
                destination.unlink(missing_ok=True)
                continue
            segments.append({"identity": identity, "file": filename})
            seen.add(identity)
            changed = True
        return changed

    async def _save_recorded_live_subtitles(self) -> None:
        if not self.task.output_path or not getattr(settings, "download_subtitles", True):
            return
        state = self._load_live_subtitle_state()
        cache_root = (self._task_dir() / "live-subtitles").resolve()
        output = Path(self.task.output_path)
        base = output.with_suffix("")
        used_labels: set[str] = set()
        saved = 0
        for position, (key, track_state) in enumerate(state.get("tracks", {}).items(), 1):
            if not isinstance(track_state, dict):
                continue
            texts: list[str] = []
            track_dir = (cache_root / key).resolve()
            for item in track_state.get("segments", []):
                if not isinstance(item, dict):
                    continue
                try:
                    path = (track_dir / str(item.get("file") or "")).resolve()
                    if not path.is_relative_to(track_dir) or not path.is_file():
                        continue
                    texts.append(path.read_text(encoding="utf-8-sig", errors="replace"))
                except (OSError, ValueError):
                    continue
            merged = merge_webvtt_segments(texts)
            if not has_cues(merged):
                continue
            label = self._subtitle_label(track_state, position, used_labels)
            vtt_path = base.with_name(f"{base.name}.{label}.vtt")
            vtt_path.write_text(merged, encoding="utf-8")
            vtt_path.with_suffix(".srt").write_text(
                webvtt_to_srt(merged), encoding="utf-8"
            )
            saved += 1
            self._log(f"[subtitles] 已保存直播字幕: {vtt_path.name}")
        if saved:
            self._log(f"[subtitles] 共保存 {saved} 条直播字幕轨道")

    def _subtitle_label(self, track: dict, position: int, used: set[str]) -> str:
        raw = str(track.get("language") or track.get("name") or f"sub{position}")
        label = sanitize_filename(raw).strip(". ") or f"sub{position}"
        if track.get("forced"):
            label += ".forced"
        candidate = label
        suffix = 2
        while candidate.lower() in used:
            candidate = f"{label}.{suffix}"
            suffix += 1
        used.add(candidate.lower())
        return candidate

    async def _download_subtitles(self, headers: dict[str, str]) -> None:
        """Save each subtitle rendition as sidecar .vtt and .srt files.

        Runs after the video is merged and verified, so every failure here
        only logs — the downloaded video is never put at risk.
        """
        tracks = self._subtitle_tracks
        if not tracks or not getattr(settings, "download_subtitles", True):
            return
        if not self.task.output_path:
            return
        output = Path(self.task.output_path)
        base = output.with_suffix("")
        used_labels: set[str] = set()
        saved = 0
        try:
            async with _create_hls_client(
                2,
                self.task.url,
                bool(self.task.engine_state.get("browser_originated")),
            ) as client:
                for position, track in enumerate(tracks, 1):
                    label = self._subtitle_label(track, position, used_labels)
                    try:
                        texts = await self._fetch_subtitle_track(
                            client, track, headers
                        )
                        merged = merge_webvtt_segments(texts)
                        if not has_cues(merged):
                            self._log(f"[subtitles] 字幕 {label} 没有有效内容，跳过")
                            continue
                        vtt_path = base.with_name(f"{base.name}.{label}.vtt")
                        vtt_path.write_text(merged, encoding="utf-8")
                        srt_path = vtt_path.with_suffix(".srt")
                        srt_path.write_text(webvtt_to_srt(merged), encoding="utf-8")
                        saved += 1
                        self._log(f"[subtitles] 已保存字幕: {vtt_path.name} / {srt_path.name}")
                    except asyncio.CancelledError:
                        raise
                    except Exception as exc:
                        self._log(f"[subtitles] 字幕 {label} 下载失败: {exc}")
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            self._log(f"[subtitles] 字幕处理失败: {exc}")
        if saved:
            self._log(f"[subtitles] 共保存 {saved} 条字幕轨道")

    async def _fetch_subtitle_track(
        self,
        client: Any,
        track: dict,
        headers: dict[str, str],
    ) -> list[str]:
        url = str(track["uri"])

        async def load_playlist():
            # Live manifests are sliding windows. Explicitly bypass caches on
            # every poll; otherwise a proxy/CDN can repeatedly return the same
            # old response and the recorder eventually reports "no new
            # segments" even though the stream is still publishing.
            request_headers = dict(headers)
            request_headers["Cache-Control"] = "no-cache, no-store, max-age=0"
            request_headers["Pragma"] = "no-cache"
            response = await client.get(url, headers=self._headers(url, request_headers))
            response.raise_for_status()
            return response

        response = await self._retry_control_request(
            load_playlist,
            stage="verifying",
            url=url,
            label="字幕清单",
        )
        text = response.text
        if text.lstrip("﻿ \t\r\n").startswith("WEBVTT"):
            # The rendition URI may point straight at a single VTT document.
            return [text]
        final_url = str(getattr(response, "url", "") or url)
        parsed = parse_m3u8(final_url, text)
        if parsed["type"] != "media":
            raise RuntimeError("字幕清单不是媒体清单")
        texts: list[str] = []
        fetch_dir = self._task_dir() / "subtitle-fetch"
        fetch_dir.mkdir(parents=True, exist_ok=True)
        for position, segment in enumerate(parsed["segments"]):
            destination = fetch_dir / f"{position:06d}.vtt"
            try:
                async def download_current_subtitle_segment() -> None:
                    await self._download_subtitle_segment(
                        client, segment, headers, destination
                    )

                await self._retry_control_request(
                    download_current_subtitle_segment,
                    stage="verifying",
                    url=segment["url"],
                    label="字幕分片",
                )
                texts.append(
                    destination.read_text(encoding="utf-8-sig", errors="replace")
                )
            finally:
                destination.unlink(missing_ok=True)
        return texts

    async def _download_subtitle_segment(
        self,
        client: Any,
        segment: dict,
        headers: dict[str, str],
        destination: Path,
    ) -> None:
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.unlink(missing_ok=True)
        key_info = segment.get("key")
        if not key_info:
            await self._download_resource(
                client,
                str(segment["url"]),
                destination,
                headers,
                segment.get("byte_range"),
            )
            return
        encrypted = destination.with_name(destination.name + ".enc")
        try:
            await self._download_resource(
                client,
                str(segment["url"]),
                encrypted,
                headers,
                segment.get("byte_range"),
            )
            key = await self._fetch_key(client, str(key_info["uri"]), headers)
            await asyncio.to_thread(
                _decrypt_aes128_file,
                encrypted,
                destination,
                key,
                key_info["iv"],
            )
        finally:
            encrypted.unlink(missing_ok=True)

    async def _download_init_maps(
        self,
        client: Any,
        segments: list[dict],
        headers: dict[str, str],
        cache: dict[tuple, Path] | None = None,
    ) -> None:
        map_dir = self._task_dir() / "maps"
        if cache is None:
            cache = {}
        for segment in segments:
            descriptor = segment.get("init_map")
            if not descriptor:
                segment["init_path"] = None
                continue
            byte_range = descriptor.get("byte_range")
            key_info = segment.get("key")
            cache_key = (
                descriptor["uri"],
                None if byte_range is None else byte_range["offset"],
                None if byte_range is None else byte_range["length"],
                None if not key_info else key_info["uri"],
                None if not key_info else key_info["iv"],
            )
            if cache_key not in cache:
                map_dir.mkdir(parents=True, exist_ok=True)
                # Content-addressed file names keep repeated calls (live
                # recording polls, resumed sessions) from reusing a slot
                # number that already belongs to a different init map.
                digest = hashlib.sha1(repr(cache_key).encode("utf-8")).hexdigest()[:16]
                destination = map_dir / f"{digest}.init"
                if not destination.exists():
                    if key_info:
                        encrypted = destination.with_name(destination.name + ".enc")
                        try:
                            await self._retry_control_request(
                                lambda: self._download_resource(
                                    client,
                                    descriptor["uri"],
                                    encrypted,
                                    headers,
                                    byte_range,
                                ),
                                stage="parsing",
                                url=descriptor["uri"],
                                label="初始化片段",
                            )
                            key = await self._fetch_key(client, key_info["uri"], headers)
                            await asyncio.to_thread(
                                _decrypt_aes128_file,
                                encrypted,
                                destination,
                                key,
                                key_info["iv"],
                            )
                        finally:
                            encrypted.unlink(missing_ok=True)
                    else:
                        await self._retry_control_request(
                            lambda: self._download_resource(
                                client,
                                descriptor["uri"],
                                destination,
                                headers,
                                byte_range,
                            ),
                            stage="parsing",
                            url=descriptor["uri"],
                            label="初始化片段",
                        )
                cache[cache_key] = destination
            segment["init_path"] = str(cache[cache_key])

    @staticmethod
    def _vod_segment_identity(segment: dict) -> str:
        """Return a non-secret identity for bytes assigned to one VOD slot.

        A numbered ``.seg`` file is safe to reuse only when the media URI,
        byte range, encryption context and initialization section still refer
        to the same resource.  Signed query values deliberately disappear via
        ``stable_request_key`` so an expired CDN signature can be refreshed
        without throwing away valid bytes.  Hashing the descriptor keeps
        access tokens and source URLs out of the checkpoint file.
        """

        def resource(value: str) -> str:
            return stable_request_key(str(value or ""), ignore_host=True)

        byte_range = segment.get("byte_range") or {}
        key_info = segment.get("key") or {}
        init_map = segment.get("init_map") or {}
        init_range = init_map.get("byte_range") or {}
        iv = key_info.get("iv")
        if isinstance(iv, bytes):
            iv_value = iv.hex()
        else:
            iv_value = str(iv or "")
        descriptor = {
            "url": resource(segment.get("url", "")),
            "range": [byte_range.get("offset"), byte_range.get("length")],
            "key": [resource(key_info.get("uri", "")), iv_value],
            "init": [
                resource(init_map.get("uri", "")),
                init_range.get("offset"),
                init_range.get("length"),
            ],
            "sequence": int(segment.get("media_sequence") or 0),
            "duration": round(float(segment.get("duration") or 0), 6),
            "discontinuity": bool(segment.get("discontinuity")),
        }
        encoded = json.dumps(
            descriptor,
            ensure_ascii=True,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
        return hashlib.sha256(encoded).hexdigest()

    def _vod_state_path(self) -> Path:
        return self._task_dir() / VOD_STATE_FILENAME

    def _write_vod_state(self) -> None:
        payload = {
            "version": VOD_STATE_VERSION,
            "segments": self._vod_resume_records,
        }
        atomic_write_text(
            self._vod_state_path(),
            json.dumps(payload, ensure_ascii=False, sort_keys=True),
        )

    def _prepare_vod_resume(self, segments: list[dict]) -> None:
        """Validate every reusable VOD segment before workers see it."""
        state_path = self._vod_state_path()
        try:
            payload = json.loads(state_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            payload = {}
        saved = payload.get("segments") if payload.get("version") == VOD_STATE_VERSION else {}
        if not isinstance(saved, dict):
            saved = {}

        identities = {
            int(segment["index"]): self._vod_segment_identity(segment)
            for segment in segments
        }
        valid: dict[str, dict[str, int | str]] = {}
        seg_dir = self._seg_dir()
        seg_dir.mkdir(parents=True, exist_ok=True)

        for index, identity in identities.items():
            path = seg_dir / f"{index:06d}.seg"
            record = saved.get(str(index))
            try:
                size = path.stat().st_size
            except OSError:
                size = 0
            expected_size = 0
            if isinstance(record, dict):
                try:
                    expected_size = int(record.get("size") or 0)
                except (TypeError, ValueError):
                    expected_size = 0
            if (
                size > 0
                and expected_size == size
                and isinstance(record, dict)
                and record.get("identity") == identity
            ):
                valid[str(index)] = {"identity": identity, "size": size}
            else:
                path.unlink(missing_ok=True)

        # Remove orphaned slots and all incomplete transport/decryption files.
        # Their names are local implementation details and never constitute a
        # completed checkpoint, even if a crash left them non-empty.
        for path in seg_dir.iterdir():
            match = re.fullmatch(r"(\d{6})\.seg(?:\..+)?", path.name)
            if not match:
                continue
            index = int(match.group(1))
            if path.suffix != ".seg" or index not in identities:
                path.unlink(missing_ok=True)

        self._vod_resume_identities = identities
        self._vod_resume_records = valid
        self._vod_resume_enabled = True
        self._write_vod_state()

    async def _checkpoint_vod_segment(self, index: int, size: int) -> None:
        if not self._vod_resume_enabled:
            return
        identity = self._vod_resume_identities.get(index)
        if not identity or size <= 0:
            return
        async with self._vod_resume_lock:
            self._vod_resume_records[str(index)] = {
                "identity": identity,
                "size": int(size),
            }
            await asyncio.to_thread(self._write_vod_state)

    def _live_state_journal_path(self) -> Path:
        return self._task_dir() / LIVE_STATE_JOURNAL_FILENAME

    def _read_live_state(self) -> dict | None:
        path = self._task_dir() / LIVE_STATE_FILENAME
        journal = self._live_state_journal_path()
        if not path.exists() and not journal.exists():
            return None
        payload: dict = {"version": 3, "total_duration": 0.0, "segments": []}
        if path.exists():
            try:
                loaded = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                return None
            if not isinstance(loaded, dict) or not isinstance(loaded.get("segments"), list):
                return None
            payload = loaded
        by_index = {
            int(item["index"]): item
            for item in payload.get("segments", [])
            if isinstance(item, dict) and str(item.get("index", "")).lstrip("-").isdigit()
        }
        if journal.exists():
            try:
                records, journal_size = read_jsonl_prefix(journal)
                accepted_offset = 0
                for event, end_offset in records:
                    if event.get("version") != 1:
                        break
                    remove = event.get("remove", [])
                    upserts = event.get("upsert", [])
                    if not isinstance(remove, list) or not isinstance(upserts, list):
                        break
                    for raw_index in remove:
                        try:
                            by_index.pop(int(raw_index), None)
                        except (TypeError, ValueError):
                            continue
                    for item in upserts:
                        if not isinstance(item, dict):
                            continue
                        try:
                            by_index[int(item["index"])] = item
                        except (KeyError, TypeError, ValueError):
                            continue
                    payload["total_duration"] = float(
                        event.get("total_duration", payload.get("total_duration", 0)) or 0
                    )
                    accepted_offset = end_offset
                if accepted_offset < journal_size:
                    truncate_durable(journal, accepted_offset)
            except OSError:
                return None
        payload["version"] = 3
        payload["segments"] = [by_index[index] for index in sorted(by_index)]
        return payload

    def _load_live_state(self) -> dict | None:
        payload = self._read_live_state()
        self._live_checkpoint_records = (
            {
                int(item["index"]): item
                for item in payload.get("segments", [])
                if isinstance(item, dict)
                and str(item.get("index", "")).lstrip("-").isdigit()
            }
            if payload is not None
            else None
        )
        self._live_checkpoint_duration = (
            float(payload.get("total_duration") or 0) if payload else 0.0
        )
        return payload

    @staticmethod
    def _live_resource_identity(entry: dict) -> str:
        saved = str(entry.get("resource_identity") or "")
        if saved:
            return saved
        stable = stable_request_key(
            str(entry.get("url") or ""),
            ignore_host=True,
        )
        return hashlib.sha256(stable.encode("utf-8")).hexdigest() if stable else ""

    def _safe_saved_init_path(self, item: dict) -> str:
        """Resolve a checkpoint map only inside this task's map directory."""
        map_dir = (self._task_dir() / "maps").resolve()
        init_name = str(item.get("init_name") or "")
        candidate = map_dir / init_name if init_name else Path(str(item.get("init_path") or ""))
        if not str(candidate):
            return ""
        try:
            resolved = candidate.resolve()
            if not resolved.is_relative_to(map_dir) or not resolved.is_file():
                return ""
        except (OSError, ValueError):
            return ""
        return str(resolved)

    def _save_live_state(
        self,
        recorded: list[dict],
        total_duration: float,
        *,
        force_compact: bool = False,
        changed_entries: list[dict] | None = None,
    ) -> None:
        seg_dir = self._seg_dir()

        def persist(entry: dict) -> dict:
            segment_path = seg_dir / f"{int(entry['index']):06d}.seg"
            try:
                size = segment_path.stat().st_size
            except OSError:
                size = 0
            return {
                "index": int(entry["index"]),
                "resource_identity": self._live_resource_identity(entry),
                "duration": float(entry.get("duration") or 0),
                "media_sequence": int(entry.get("media_sequence") or 0),
                "part_index": entry.get("part_index"),
                "is_partial": bool(entry.get("is_partial")),
                "discontinuity": bool(entry.get("discontinuity")),
                "init_name": Path(str(entry.get("init_path") or "")).name,
                "size": int(size),
            }

        destination = self._task_dir() / LIVE_STATE_FILENAME
        journal = self._live_state_journal_path()
        old = self._live_checkpoint_records
        if old is None:
            previous = self._read_live_state()
            old = {
                int(item["index"]): item
                for item in (previous or {}).get("segments", [])
                if isinstance(item, dict)
                and str(item.get("index", "")).lstrip("-").isdigit()
            }
            self._live_checkpoint_duration = (
                float(previous.get("total_duration") or 0) if previous else 0.0
            )
        if changed_entries is None:
            new = {int(entry["index"]): persist(entry) for entry in recorded}
            removed = sorted(set(old) - set(new))
            upserts = [new[index] for index in sorted(new) if old.get(index) != new[index]]
        else:
            new = dict(old)
            upserts = []
            for entry in changed_entries:
                item = persist(entry)
                index = int(item["index"])
                if new.get(index) != item:
                    new[index] = item
                    upserts.append(item)
            removed = []
        duration = float(total_duration or 0)

        def payload() -> dict:
            return {
                "version": 3,
                "total_duration": duration,
                "segments": [new[index] for index in sorted(new)],
            }

        if force_compact or not destination.exists():
            snapshot = payload()
            atomic_write_text(destination, json.dumps(snapshot, ensure_ascii=False))
            journal.unlink(missing_ok=True)
        elif upserts or removed or self._live_checkpoint_duration != duration:
            line = json.dumps({
                "version": 1,
                "total_duration": duration,
                "remove": removed,
                "upsert": upserts,
            }, ensure_ascii=False, separators=(",", ":")) + "\n"
            journal.parent.mkdir(parents=True, exist_ok=True)
            with journal.open("a", encoding="utf-8", newline="\n") as stream:
                stream.write(line)
                stream.flush()
                os.fsync(stream.fileno())
            if journal.stat().st_size >= max(
                LIVE_STATE_JOURNAL_MIN_COMPACT_BYTES,
                destination.stat().st_size * 2,
            ):
                atomic_write_text(destination, json.dumps(payload(), ensure_ascii=False))
                journal.unlink(missing_ok=True)
        self._live_checkpoint_records = new
        self._live_checkpoint_duration = duration

    def _restore_live_segments(
        self,
        saved_state: dict,
        recorded: list[dict],
    ) -> float:
        """Rebuild the recorded segment list from a previous session.

        Only segments whose files are still complete are kept, so a crash in
        the middle of a batch shrinks the recording instead of corrupting the
        final merge.  A dropped segment marks the next kept one with a
        discontinuity so the playback timeline reflects the gap.
        """
        seg_dir = self._seg_dir()
        total_duration = 0.0
        gap = False
        for item in saved_state.get("segments", []):
            try:
                index = int(item["index"])
            except (KeyError, TypeError, ValueError):
                continue
            segment_path = seg_dir / f"{index:06d}.seg"
            if not segment_path.exists() or segment_path.stat().st_size == 0:
                gap = True
                continue
            expected_size = int(item.get("size") or 0)
            if expected_size and segment_path.stat().st_size != expected_size:
                gap = True
                continue
            had_init = bool(item.get("init_name") or item.get("init_path"))
            init_path = self._safe_saved_init_path(item) if had_init else ""
            if had_init and not init_path:
                gap = True
                continue
            entry = {
                "index": index,
                # Old checkpoints can still resume once; the next atomic save
                # migrates them to a token-free resource identity.
                "url": str(item.get("url") or ""),
                "resource_identity": str(item.get("resource_identity") or ""),
                "duration": float(item.get("duration") or 0),
                "media_sequence": int(item.get("media_sequence") or 0),
                "part_index": item.get("part_index"),
                "is_partial": bool(item.get("is_partial")),
                "discontinuity": bool(item.get("discontinuity")) or gap,
                "init_path": init_path or None,
                "init_map": None,
                "key": None,
                "byte_range": None,
            }
            gap = False
            recorded.append(entry)
            total_duration += entry["duration"]
        recorded.sort(key=lambda entry: entry["index"])
        return total_duration

    def _compact_recorded(self, recorded: list[dict]) -> int:
        """Renumber recorded segments 0..n-1, renaming files to match.

        The playback service requires plan index == list position to map
        segment URLs onto disk files, so a recorded list must never keep the
        holes left by dropped segments.  Holes only ever close downward, so
        renaming in ascending order is collision-free.
        """
        seg_dir = self._seg_dir()
        for position, entry in enumerate(recorded):
            old = int(entry["index"])
            if old == position:
                continue
            source = seg_dir / f"{old:06d}.seg"
            destination = seg_dir / f"{position:06d}.seg"
            if source.exists():
                try:
                    source.replace(destination)
                except OSError:
                    # An open playback handle can block the rename on
                    # Windows; publish a flushed copy atomically so the next
                    # checkpoint cannot point at a half-copied segment.
                    temporary = destination.with_name(destination.name + ".compact.tmp")
                    try:
                        shutil.copyfile(source, temporary)
                        durable_replace(temporary, destination)
                        source.unlink(missing_ok=True)
                    finally:
                        temporary.unlink(missing_ok=True)
            entry["index"] = position
        return len(recorded)

    def _purge_orphan_live_segments(self, next_index: int) -> None:
        """Delete segment files above the last persisted index.

        A crash between a segment download and the next live_state.json write
        leaves files whose indexes will be reassigned to *new* media
        sequences; without this purge the exists-shortcut in
        _download_one_segment would silently splice stale pre-crash bytes
        into the new positions.
        """
        for stray in self._seg_dir().glob("*.seg"):
            try:
                index = int(stray.stem)
            except ValueError:
                continue
            if index >= next_index:
                stray.unlink(missing_ok=True)

    async def _reload_live_playlist(
        self,
        client: Any,
        url: str,
        headers: dict[str, str],
        previous: dict | None = None,
    ) -> dict:
        async def load_playlist():
            # Live manifests are sliding windows. Explicitly bypass caches on
            # every poll; otherwise a proxy/CDN can repeatedly return the same
            # old response and the recorder eventually reports "no new
            # segments" even though the stream is still publishing.
            request_headers = dict(headers)
            request_headers["Cache-Control"] = "no-cache, no-store, max-age=0"
            request_headers["Pragma"] = "no-cache"
            request_url = _blocking_reload_url(url, previous)
            response = await client.get(
                request_url,
                headers=self._headers(request_url, request_headers),
            )
            # A few CDN edges advertise CAN-BLOCK-RELOAD but reject the query
            # when a request reaches a non-LL-HLS edge.  Do not turn that
            # deployment quirk into a failed recording: retry the ordinary
            # no-cache URL once, while retaining the same auth headers.
            if request_url != url and response.status_code in {400, 404, 412, 422}:
                await response.aclose()
                response = await client.get(url, headers=self._headers(url, request_headers))
            response.raise_for_status()
            return response

        response = await self._retry_control_request(
            load_playlist,
            stage="downloading_segments",
            url=url,
            label="直播清单",
        )
        final_url = str(getattr(response, "url", "") or url)
        parsed = parse_m3u8(final_url, response.text)
        if parsed["type"] != "media":
            raise RuntimeError("直播清单刷新后不再是媒体清单")
        parsed = filter_ad_segments(
            parsed,
            enabled=bool(getattr(settings, "skip_ad_segments", True)),
        )
        skipped = int(parsed.get("ad_segments_skipped") or 0)
        if skipped:
            self._log(f"[recording] 已跳过 {skipped} 个 HLS 广告标记分片")
        parsed["final_url"] = final_url
        return parsed

    async def _live_wait(self, seconds: float) -> None:
        deadline = asyncio.get_running_loop().time() + max(0.0, seconds)
        while asyncio.get_running_loop().time() < deadline:
            if self._is_canceled() or self._is_pausing():
                return
            await asyncio.sleep(0.2)

    async def _download_live_batch(
        self,
        client: Any,
        batch: list[dict],
        headers: dict[str, str],
        pending_gap: bool = False,
    ) -> tuple[list[dict], bool]:
        """Download one poll's worth of new segments, skipping hard failures.

        A live window keeps sliding, so a segment that still fails after the
        shared retry policy is dropped (with a discontinuity marker on the
        next kept segment) instead of aborting the whole recording.  The gap
        flag is carried across batches so a failure at a batch boundary still
        marks the next kept segment.
        """
        semaphore = asyncio.Semaphore(LIVE_BATCH_CONCURRENCY)
        outcomes: dict[int, bool] = {}

        async def fetch(entry: dict) -> None:
            async with semaphore:
                if self._is_canceled() or self._is_pausing():
                    outcomes[entry["index"]] = False
                    return
                try:
                    outcomes[entry["index"]] = bool(
                        await self._download_one_segment(client, entry, headers)
                    )
                except asyncio.CancelledError:
                    # A stop request during an AES-key fetch must not abort
                    # the recording; drop the segment and let the loop
                    # finalize what was captured.
                    if self._is_pausing() and not self._is_canceled():
                        outcomes[entry["index"]] = False
                        return
                    raise
                except Exception as exc:
                    outcomes[entry["index"]] = False
                    self.task.progress.failed_segments += 1
                    self.task.progress.last_worker_error = (
                        f"[{entry['index']}] {str(exc)[:120]}"
                    )
                    self._log(f"[segment {entry['index']}] 直播分片下载失败，已跳过: {exc}")

        await asyncio.gather(*(fetch(entry) for entry in batch))

        kept: list[dict] = []
        gap = pending_gap
        for entry in batch:
            if outcomes.get(entry["index"]):
                if gap:
                    entry["discontinuity"] = True
                    gap = False
                kept.append(entry)
            else:
                gap = True
        return kept, gap

    async def _record_live(
        self,
        client: Any,
        parsed: dict,
        headers: dict[str, str],
        saved_state: dict | None,
    ) -> tuple[list[dict], float] | None:
        task = self.task
        task_dir = self._task_dir()
        playlist_url = str(parsed.get("final_url") or parsed["url"])
        target_duration = max(1.0, float(parsed.get("target_duration") or 6.0))
        max_minutes = max(0, int(getattr(settings, "live_record_max_minutes", 0) or 0))
        max_seconds = max_minutes * 60.0
        stall_window = max(
            LIVE_STALL_MIN_SECONDS,
            LIVE_STALL_TARGET_MULTIPLIER * target_duration,
        )

        recorded: list[dict] = []
        init_cache: dict[tuple, Path] = {}
        total_duration = 0.0
        session_boundary = False
        if saved_state:
            total_duration = self._restore_live_segments(saved_state, recorded)
            session_boundary = bool(recorded)
            if recorded:
                self._log(
                    f"[recording] 继续上次录制：已有 {len(recorded)} 个分片"
                    f"（{_format_clock(total_duration)}）"
                )
        next_index = await asyncio.to_thread(self._compact_recorded, recorded)
        self._purge_orphan_live_segments(next_index)
        if recorded:
            # Compaction may have renamed segment files; persist the new
            # index mapping immediately so a crash before the next batch
            # cannot resurrect the stale layout.
            await asyncio.to_thread(self._save_live_state, recorded, total_duration)
        recorded_identities = {
            self._live_resource_identity(entry)
            for entry in recorded
            if self._live_resource_identity(entry)
        }
        last_sequence = max(
            (
                int(entry.get("media_sequence") or 0)
                for entry in recorded
                if not entry.get("is_partial")
            ),
            default=-1,
        )
        partial_sequences = {
            int(entry.get("media_sequence") or 0)
            for entry in recorded
            if entry.get("is_partial")
        }

        self.tracker.start(len(recorded))
        self._completed_count = 0
        for entry in recorded:
            segment_path = self._seg_dir() / f"{entry['index']:06d}.seg"
            self.tracker.add_completed(segment_path.stat().st_size)
            self._completed_count += 1

        task.status = TaskStatus.DOWNLOADING_SEGMENTS
        task.progress.total_segments = len(recorded)
        task.progress.max_workers = LIVE_BATCH_CONCURRENCY
        task.progress.connection_status = "running"
        task.progress.media_duration = total_duration
        if recorded:
            await asyncio.to_thread(write_playback_plan, task_dir, recorded, total_duration)
            self._refresh_playback_progress()
        self._set_stage("recording", "直播流录制中，停止录制后自动合并")

        loop = asyncio.get_running_loop()
        last_new_segment = loop.time()
        current = parsed
        playlist_loaded_at = loop.time()
        finish_reason = ""
        pending_gap = False

        while True:
            if self._is_canceled():
                raise asyncio.CancelledError
            if self._is_pausing():
                finish_reason = "已停止录制"
                break

            window = [
                segment
                for segment in current.get("segments", [])
                if segment.get("url")
            ]
            window_sequences = [
                int(segment.get("media_sequence") or 0) for segment in window
            ]
            complete_window_sequences = [
                int(segment.get("media_sequence") or 0)
                for segment in window
                if not segment.get("is_partial")
            ]
            # An encoder restart resets EXT-X-MEDIA-SEQUENCE; treat the whole
            # window as fresh content instead of skipping it until the stall
            # timeout ends a stream that is actually still live.  A stale CDN
            # edge replaying an old window looks identical sequence-wise, so
            # a reset also requires at least one URL never recorded before.
            epoch_reset = bool(
                window_sequences
                and last_sequence >= 0
                and max(window_sequences) < last_sequence - len(window_sequences)
                and any(
                    self._live_resource_identity(segment) not in recorded_identities
                    for segment in window
                )
            )
            if epoch_reset:
                self._log(
                    "[recording] 直播序号已重置"
                    f"（{last_sequence} → {max(window_sequences)}），继续录制新片段"
                )
                if not complete_window_sequences:
                    # A restarted encoder can initially expose only PARTs.
                    # Reset the completed cursor to just before their parent
                    # sequence; otherwise every new PART in that parent would
                    # be mistaken for another epoch reset and gain a false
                    # discontinuity.
                    last_sequence = min(window_sequences) - 1
            new_batch: list[dict] = []
            projected = total_duration
            for segment in window:
                sequence = int(segment.get("media_sequence") or 0)
                identity = self._live_resource_identity(segment)
                if not epoch_reset:
                    if segment.get("is_partial"):
                        if sequence <= last_sequence or identity in recorded_identities:
                            continue
                    elif sequence <= last_sequence:
                        continue
                if (
                    epoch_reset
                    and identity in recorded_identities
                ):
                    continue
                if max_seconds and projected >= max_seconds:
                    # An event playlist can list hours of backlog in a single
                    # window; stop queueing at the cap so the output actually
                    # honors the configured limit.
                    break
                entry = dict(segment)
                if not entry.get("is_partial") and sequence in partial_sequences:
                    # A completed parent contains the same samples as its
                    # previously published PART objects. Download it into a
                    # new slot first; only after success do we remove the
                    # partial files and replace them in the timeline. This
                    # avoids duplicate time without losing late parts that
                    # appeared between playlist polls.
                    entry["replaces_partial_sequence"] = sequence
                entry["index"] = next_index
                next_index += 1
                if session_boundary or (epoch_reset and not new_batch):
                    entry["discontinuity"] = True
                    session_boundary = False
                projected += float(entry.get("duration") or 0)
                new_batch.append(entry)
            if new_batch:
                task.progress.total_segments = len(recorded) + len(new_batch)
                try:
                    await self._download_init_maps(
                        client, new_batch, headers, cache=init_cache
                    )
                except asyncio.CancelledError:
                    # A stop request during a control fetch surfaces as a
                    # cancellation; it must finalize the recording, not leave
                    # the task in an interrupted state.
                    if (
                        self._is_pausing()
                        and not self._is_canceled()
                        and not _externally_cancelled()
                    ):
                        finish_reason = "已停止录制"
                        break
                    raise
                except Exception as exc:
                    if recorded:
                        self._log(f"[recording] 初始化片段获取失败，结束录制: {exc}")
                        finish_reason = "直播源资源已不可用，录制结束"
                        break
                    raise
                kept, pending_gap = await self._download_live_batch(
                    client, new_batch, headers, pending_gap
                )
                if kept:
                    # Commit the media-sequence cursor only after bytes are
                    # safely on disk.  Signed live segment URLs can expire
                    # between the manifest poll and the first GET.  Advancing
                    # on observation made an all-failed first batch disappear
                    # forever, even when the next manifest refreshed the URL,
                    # leaving the task at 0 seconds until the stall timeout.
                    # Once a later segment succeeds it is correct to advance
                    # past an unrecoverable gap and keep recording live media.
                    kept_complete_sequences = [
                        int(entry.get("media_sequence") or 0)
                        for entry in kept
                        if not entry.get("is_partial")
                    ]
                    if kept_complete_sequences:
                        if epoch_reset:
                            last_sequence = max(kept_complete_sequences)
                        else:
                            last_sequence = max(last_sequence, max(kept_complete_sequences))
                    replacement_sequences = {
                        int(entry["replaces_partial_sequence"])
                        for entry in kept
                        if entry.get("replaces_partial_sequence") is not None
                    }
                    if replacement_sequences:
                        survivors = []
                        removed_duration = 0.0
                        replacement_boundaries: set[int] = set()
                        for existing in recorded:
                            sequence = int(existing.get("media_sequence") or 0)
                            if existing.get("is_partial") and sequence in replacement_sequences:
                                removed_duration += float(existing.get("duration") or 0)
                                if existing.get("discontinuity"):
                                    replacement_boundaries.add(sequence)
                                recorded_identities.discard(
                                    self._live_resource_identity(existing)
                                )
                                (self._seg_dir() / f"{int(existing['index']):06d}.seg").unlink(
                                    missing_ok=True
                                )
                            else:
                                survivors.append(existing)
                        recorded = survivors
                        total_duration = max(0.0, total_duration - removed_duration)
                        partial_sequences.difference_update(replacement_sequences)
                        for replacement in kept:
                            replacement_sequence = replacement.get(
                                "replaces_partial_sequence"
                            )
                            if (
                                replacement_sequence is not None
                                and int(replacement_sequence) in replacement_boundaries
                            ):
                                replacement["discontinuity"] = True
                    recorded.extend(kept)
                    recorded_identities.update(
                        self._live_resource_identity(entry)
                        for entry in kept
                        if self._live_resource_identity(entry)
                    )
                    partial_sequences.update(
                        int(entry.get("media_sequence") or 0)
                        for entry in kept
                        if entry.get("is_partial")
                    )
                    total_duration += sum(
                        float(entry.get("duration") or 0) for entry in kept
                    )
                    last_new_segment = loop.time()
                    # Dropped segments burn indexes; renumber so the playback
                    # plan keeps its index == position invariant.
                    next_index = await asyncio.to_thread(self._compact_recorded, recorded)
                    self._purge_orphan_live_segments(next_index)
                    self.tracker.total = len(recorded)
                    task.progress.media_duration = total_duration
                    await asyncio.to_thread(
                        write_playback_plan,
                        task_dir,
                        recorded,
                        total_duration,
                        changed_segments=(None if replacement_sequences else kept),
                    )
                    await asyncio.to_thread(
                        self._save_live_state,
                        recorded,
                        total_duration,
                        changed_entries=(None if replacement_sequences else kept),
                    )
                    self._refresh_playback_progress()
                    self._emit_progress()
                    task.progress.connection_status = "running"
                    self._set_stage(
                        "recording",
                        f"直播录制中：已录制 {_format_clock(total_duration)}"
                        f"（{len(recorded)} 分片）",
                    )
                task.progress.total_segments = len(recorded)

            if not current.get("is_live", True):
                finish_reason = "直播已结束"
                break
            if max_seconds and total_duration >= max_seconds:
                finish_reason = f"已达到录制时长上限 {max_minutes} 分钟"
                break
            if loop.time() - last_new_segment > stall_window:
                if recorded:
                    finish_reason = "直播源已停止更新，自动结束录制"
                    break
                if task.progress.failed_segments:
                    # Do not collapse an authentication/segment CDN failure
                    # into the misleading "playlist stopped" message.  A
                    # signed stream can return a perfectly valid playlist
                    # while every media URI is already expired; surfacing the
                    # last worker error tells the user to refresh the source
                    # page instead of retrying an unchanged URL forever.
                    detail = str(task.progress.last_worker_error or "首批直播分片下载失败")[:240]
                    raise RuntimeError(f"直播首批分片下载失败：{detail}")
                raise RuntimeError("直播清单长时间没有新分片，直播源可能已停止")

            delay = _live_reload_delay(current, bool(new_batch))
            # Keep a strict playlist cadence: segment downloads and state
            # writes already consumed part of the interval. Adding a fresh
            # full delay here made LL-HLS polling drift farther behind on every
            # iteration, eventually allowing the PART window to slide away.
            elapsed = max(0.0, loop.time() - playlist_loaded_at)
            await self._live_wait(max(0.0, delay - elapsed))
            if self._is_canceled():
                raise asyncio.CancelledError
            if self._is_pausing():
                finish_reason = "已停止录制"
                break

            try:
                current = await self._reload_live_playlist(
                    client, playlist_url, headers, current
                )
                playlist_loaded_at = loop.time()
                playlist_url = str(current.get("final_url") or playlist_url)
            except asyncio.CancelledError:
                if (
                    self._is_pausing()
                    and not self._is_canceled()
                    and not _externally_cancelled()
                ):
                    finish_reason = "已停止录制"
                    break
                raise
            except Exception as exc:
                if recorded:
                    self._log(f"[recording] 直播清单刷新失败，结束录制: {exc}")
                    finish_reason = "直播清单已不可用，录制结束"
                    break
                raise

        if not recorded:
            if task.pause_event is not None:
                task.pause_event.clear()
            task.status = TaskStatus.PAUSED
            task.progress.connection_status = "idle"
            self._set_stage("paused", "已停止，尚未录制到内容，可重新开始")
            return None

        # A stop request can land mid-batch; keep only complete files so the
        # final merge never trips over a truncated tail segment.
        seg_dir = self._seg_dir()
        final_segments = []
        dropped = 0
        for entry in recorded:
            segment_path = seg_dir / f"{entry['index']:06d}.seg"
            if segment_path.exists() and segment_path.stat().st_size > 0:
                final_segments.append(entry)
            else:
                dropped += 1
        if dropped:
            self._log(f"[recording] 丢弃 {dropped} 个未完成分片")
        if not final_segments:
            raise RuntimeError("直播录制没有可用分片")
        await asyncio.to_thread(self._compact_recorded, final_segments)

        final_duration = sum(
            float(entry.get("duration") or 0) for entry in final_segments
        )
        task.progress.total_segments = len(final_segments)
        task.progress.media_duration = final_duration
        await asyncio.to_thread(
            write_playback_plan,
            task_dir,
            final_segments,
            final_duration,
            force_compact=True,
        )
        await asyncio.to_thread(
            self._save_live_state,
            final_segments,
            final_duration,
            force_compact=True,
        )
        self._set_stage(
            "recording",
            f"{finish_reason}，共录制 {_format_clock(final_duration)}，正在合并",
        )
        return final_segments, final_duration

    async def _download_segments(
        self,
        client: Any,
        segments: list[dict],
        headers: dict[str, str],
        concurrency: int,
    ) -> bool:
        self.tracker.start(len(segments))
        self._completed_count = 0
        self._failed_indexes = []
        self._last_segment_error = None
        self.task.progress.failed_segments = 0
        self._retry_window = SharedRetryWindow()
        pending: dict[int, dict] = {}
        for segment in segments:
            destination = self._seg_dir() / f"{segment['index']:06d}.seg"
            if destination.exists() and destination.stat().st_size > 0:
                self.tracker.add_completed(destination.stat().st_size)
                self._completed_count += 1
            else:
                pending[segment["index"]] = segment

        # Do not serialize a playback warm-up here.  Every configured worker
        # must immediately own an independent in-flight segment request; with
        # HTTP/1.1 this creates multiple simultaneous CDN connections instead
        # of repeatedly using one rate-limited connection.  claim_segment()
        # still hands out the lowest indexes first, so the playable prefix is
        # prepared by the same concurrent pool.
        claim_lock = asyncio.Lock()

        async def claim_segment() -> dict | None:
            async with claim_lock:
                if not pending:
                    return None
                priority = self._playback_priority_index
                if priority is not None:
                    forward = [index for index in pending if index >= priority]
                    index = min(forward) if forward else min(pending)
                else:
                    index = min(pending)
                return pending.pop(index)

        async def worker() -> None:
            while True:
                if self._is_canceled() or self._is_pausing():
                    return
                segment = await claim_segment()
                if segment is None:
                    return
                index = segment["index"]
                self.task.progress.active_workers += 1
                self.task.progress.active_slots += 1
                self.task.progress.active_segment_indexes.append(index)
                self._publish()
                try:
                    completed = await self._download_one_segment(client, segment, headers)
                    if completed:
                        self._refresh_playback_progress()
                except asyncio.CancelledError:
                    raise
                except Exception as exc:
                    failure = as_download_error(
                        exc,
                        stage="downloading_segments",
                        url=segment["url"],
                        attempt=MAX_RETRIES,
                        task_context=self.task,
                    )
                    if self._last_segment_error is None:
                        self._last_segment_error = failure
                    self._failed_indexes.append(index)
                    self.task.progress.failed_segments = len(self._failed_indexes)
                    self.task.progress.last_worker_error = f"[{index}] {str(exc)[:120]}"
                    self._log(f"[segment {index}] 下载失败: {exc}")
                finally:
                    self.task.progress.active_workers -= 1
                    self.task.progress.active_slots -= 1
                    if index in self.task.progress.active_segment_indexes:
                        self.task.progress.active_segment_indexes.remove(index)
                    self._emit_progress()

        workers = [
            asyncio.create_task(worker())
            for _ in range(min(max(1, concurrency), len(pending)))
        ]
        try:
            await asyncio.gather(*workers)
        finally:
            for worker_task in workers:
                if not worker_task.done():
                    worker_task.cancel()
            await asyncio.gather(*workers, return_exceptions=True)

        if not pending and not self._is_pausing() and not self._is_canceled():
            self._playback_priority_index = None
            self.task.playback_seek_index = None
        return not self._is_canceled() and not self._is_pausing()

    async def _download_one_segment(
        self,
        client: Any,
        segment: dict,
        headers: dict[str, str],
    ) -> bool:
        index = segment["index"]
        destination = self._seg_dir() / f"{index:06d}.seg"
        if destination.exists() and destination.stat().st_size > 0:
            self.tracker.add_completed(destination.stat().st_size)
            self._completed_count += 1
            return True

        last_error: Exception | None = None
        attempts_made = 0
        for attempt in range(MAX_RETRIES):
            attempts_made = attempt + 1
            if self._is_canceled() or self._is_pausing():
                return False
            if not await self._retry_window.wait(lambda: self._is_canceled() or self._is_pausing()):
                return False
            self._clear_rate_limit_notice()
            try:
                key_info = segment.get("key")
                if key_info:
                    encrypted = destination.with_name(destination.name + ".enc")
                    await self._download_resource(
                        client,
                        segment["url"],
                        encrypted,
                        headers,
                        segment.get("byte_range"),
                    )
                    key = await self._fetch_key(client, key_info["uri"], headers)
                    await asyncio.to_thread(
                        _decrypt_aes128_file,
                        encrypted,
                        destination,
                        key,
                        key_info["iv"],
                    )
                    encrypted.unlink(missing_ok=True)
                else:
                    await self._download_resource(
                        client,
                        segment["url"],
                        destination,
                        headers,
                        segment.get("byte_range"),
                    )
                size = destination.stat().st_size
                await self._checkpoint_vod_segment(index, size)
                self.tracker.add_completed(size)
                self._completed_count += 1
                if (
                    self._completed_count % 10 == 0
                    or self._completed_count == self.task.progress.total_segments
                ) and self.task.stage != "recording":
                    # The live loop owns the stage line while recording.
                    snapshot = self.tracker.snapshot()
                    self._set_stage(
                        "downloading_segments",
                        f"{self._completed_count}/{self.task.progress.total_segments} 分片 "
                        f"{snapshot['speed'] / 1024:.0f} KB/s",
                    )
                return True
            except asyncio.CancelledError:
                if self._should_propagate_cancellation():
                    raise
                # See _retry_control_request: this is a curl transport abort,
                # not a user pause.  Preserve the partial-file cleanup and
                # retry it through the normal transient-network policy.
                last_error = self._unexpected_request_cancellation()
                self.task.progress.reconnect_count += 1
                self.task.progress.connection_status = "reconnecting"
                destination.unlink(missing_ok=True)
                destination.with_name(destination.name + ".tmp").unlink(missing_ok=True)
                if attempt < MAX_RETRIES - 1:
                    self._log(
                        f"[segment {index}] 第 {attempt + 1}/{MAX_RETRIES} 次请求被网络层中断，正在重试"
                    )
                    await asyncio.sleep(
                        retry_delay_seconds(last_error, min(2**attempt, 10))
                    )
            except Exception as exc:
                last_error = exc
                self.task.progress.reconnect_count += 1
                self.task.progress.connection_status = "reconnecting"
                destination.unlink(missing_ok=True)
                destination.with_name(destination.name + ".tmp").unlink(missing_ok=True)
                if not should_retry_download_error(exc):
                    break
                if attempt < MAX_RETRIES - 1:
                    self._log(
                        f"[segment {index}] 第 {attempt + 1}/{MAX_RETRIES} 次失败: {exc}"
                    )
                    delay = retry_delay_seconds(exc, min(2**attempt, 10))
                    if should_share_retry_window(exc):
                        remaining, extended = await self._retry_window.extend(delay)
                        if extended:
                            self._announce_rate_limit(remaining)
                    else:
                        await asyncio.sleep(delay)
        if last_error is None:
            raise RuntimeError(f"分片 {index} 下载失败")
        raise as_download_error(
            last_error,
            stage="downloading_segments",
            url=segment["url"],
            attempt=attempts_made,
            task_context=self.task,
        ) from last_error

    def _refresh_playback_progress(self) -> None:
        try:
            snapshot = playback_service.snapshot(
                self.task.id,
                self.task.status.value,
                self.task.output_path,
            )
        except Exception as exc:
            # Do not hide an on-disk plan error behind a missing preview action.
            # Keep the normal download stage intact and log it only once so a
            # transient filesystem race cannot flood the task log.
            diagnostic = f"{type(exc).__name__}: {str(exc)[:180]}"
            if diagnostic != self._playback_refresh_error:
                self._playback_refresh_error = diagnostic
                self._log(f"[playback] 边下边播缓冲暂不可用: {diagnostic}")
            return
        self._playback_refresh_error = ""
        progress = self.task.progress
        progress.playable_segments = snapshot.available_segments
        progress.playable_duration = snapshot.available_duration
        progress.media_duration = snapshot.total_duration

    async def _fetch_key(
        self,
        client: Any,
        url: str,
        headers: dict[str, str],
    ) -> bytes:
        if url not in self._key_cache:
            async def load_key():
                response = await client.get(url, headers=self._headers(url, headers))
                response.raise_for_status()
                return response

            response = await self._retry_control_request(
                load_key,
                stage=self.task.stage or "parsing",
                url=url,
                label="AES 密钥",
            )
            if len(response.content) != 16:
                raise ValueError(
                    f"AES-128 密钥长度必须是 16 字节，实际为 {len(response.content)}"
                )
            self._key_cache[url] = response.content
        return self._key_cache[url]

    async def _download_resource(
        self,
        client: Any,
        url: str,
        destination: Path,
        headers: dict[str, str],
        byte_range: dict | None = None,
    ) -> int:
        destination.parent.mkdir(parents=True, exist_ok=True)
        temporary = destination.with_name(destination.name + ".tmp")
        temporary.unlink(missing_ok=True)
        request_headers = self._headers(url, headers)
        expected_length = None
        if byte_range:
            start = int(byte_range["offset"])
            expected_length = int(byte_range["length"])
            end = start + expected_length - 1
            request_headers["Range"] = f"bytes={start}-{end}"

        def validate_response(response) -> None:
            if response.status_code >= 400:
                response.raise_for_status()
            if not byte_range:
                return
            if response.status_code != 206:
                raise RuntimeError(
                    f"BYTERANGE 请求需要 HTTP 206，实际为 {response.status_code}"
                )
            match = _CONTENT_RANGE_RE.match(response.headers.get("Content-Range", ""))
            if not match:
                raise RuntimeError("BYTERANGE 响应缺少有效 Content-Range")
            actual_start, actual_end = int(match.group(1)), int(match.group(2))
            if actual_start != start or actual_end != end:
                raise RuntimeError(
                    f"Content-Range 不匹配，期望 {start}-{end}，实际 "
                    f"{actual_start}-{actual_end}"
                )

        written = 0
        try:
            if hasattr(client, "download_to_file"):
                response, written = await client.download_to_file(
                    url,
                    temporary,
                    request_headers,
                    self._is_canceled,
                    self.task,
                )
                validate_response(response)
            else:
                async with client.stream("GET", url, headers=request_headers) as response:
                    validate_response(response)
                    with temporary.open("wb") as output:
                        async for chunk in response.aiter_bytes(256 * 1024):
                            if self._is_canceled():
                                raise asyncio.CancelledError
                            await throttle_bytes(len(chunk), self.task)
                            output.write(chunk)
                            written += len(chunk)

            if written == 0:
                raise RuntimeError("下载结果为空")
            if expected_length is not None and written != expected_length:
                raise RuntimeError(
                    f"BYTERANGE 长度不匹配，期望 {expected_length}，实际 {written}"
                )
            await asyncio.to_thread(durable_replace, temporary, destination)
            return written
        finally:
            temporary.unlink(missing_ok=True)

    def _emit_progress(self) -> None:
        snapshot = self.tracker.snapshot()
        progress = self.task.progress
        progress.downloaded_bytes = snapshot["downloaded_bytes"]
        progress.total_bytes = snapshot["total_bytes"]
        progress.speed_bytes_per_sec = snapshot["speed"]
        progress.eta_seconds = snapshot["eta"]
        progress.completed_segments = snapshot["completed"]
        progress.connection_status = (
            "running" if progress.active_workers else "idle"
        )
        self._publish()
