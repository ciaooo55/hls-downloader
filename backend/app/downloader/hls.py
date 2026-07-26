import asyncio
import hashlib
import json
import re
import shutil
from collections.abc import Awaitable, Callable
from datetime import datetime
from pathlib import Path
from typing import Any

import httpx
try:
    from curl_cffi.requests import AsyncSession as CurlAsyncSession
except ImportError:
    CurlAsyncSession = None
from cryptography.hazmat.primitives import padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

from ..config import settings
from ..checksum import verify_task_checksum
from ..models import Task, TaskStatus
from ..utils import sanitize_filename
from ..naming import is_generic_media_name, suggest_manifest_name
from ..request_context import build_task_headers
from .http_file import _content_disposition_filename
from .dash import DashDownloader
from .merge import merge_segments
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
from .parser import UnsupportedPlaylistError, parse_m3u8
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
LIVE_STALL_MIN_SECONDS = 90.0
LIVE_STALL_TARGET_MULTIPLIER = 6.0
LIVE_BATCH_CONCURRENCY = 3
LIVE_MAX_POLL_SECONDS = 10.0


class _BrowserHLSClient:
    def __init__(self, concurrency: int) -> None:
        self._session = CurlAsyncSession(
            max_clients=concurrency + 4,
            default_headers=False,
            http_version="v1",
            timeout=(10, 60),
            allow_redirects=True,
        )

    async def __aenter__(self):
        await self._session.__aenter__()
        return self

    async def __aexit__(self, *args):
        return await self._session.__aexit__(*args)

    async def get(self, url: str, **kwargs):
        kwargs.setdefault("impersonate", _browser_impersonation(kwargs.get("headers")))
        return await self._session.get(url, **kwargs)

    async def download_to_file(
        self,
        url: str,
        destination: Path,
        headers: dict[str, str],
        cancel_check,
        task=None,
    ) -> tuple[Any, int]:
        written = 0
        response = await self._session.get(
            url,
            headers=headers,
            stream=True,
            impersonate=_browser_impersonation(headers),
        )
        try:
            with destination.open("wb") as output:
                async for chunk in response.aiter_content():
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
                await response.aclose()
        return response, written


def _browser_impersonation(headers: dict[str, str] | None) -> str:
    values = {str(name).lower(): str(value) for name, value in dict(headers or {}).items()}
    user_agent = values.get("user-agent", "").lower()
    if "edg/" in user_agent or "chrome/" in user_agent or "chromium/" in user_agent:
        return "chrome"
    if "safari/" in user_agent and "chrome/" not in user_agent:
        return "safari"
    return "firefox"


def _create_hls_client(concurrency: int):
    if CurlAsyncSession is not None:
        return _BrowserHLSClient(concurrency)
    limits = httpx.Limits(
        max_connections=concurrency + 4,
        max_keepalive_connections=concurrency + 2,
    )
    return httpx.AsyncClient(
        timeout=SEG_TIMEOUT,
        follow_redirects=True,
        limits=limits,
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
        temporary.replace(destination)
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
                raise
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
            parsed = parse_m3u8(final_url, response.text, self.task.selected_video)
            manifest_title = manifest_title or parsed.get("title", "")
            response_filename = response_filename or _content_disposition_filename(
                response.headers.get("content-disposition", "")
            )
            if parsed["type"] == "media":
                parsed["content"] = response.text
                parsed["title"] = manifest_title
                parsed["response_filename"] = response_filename
                parsed["final_url"] = final_url
                parsed["external_audio"] = external_audio
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
        task_dir = self._task_dir()
        seg_dir = self._seg_dir()
        seg_dir.mkdir(parents=True, exist_ok=True)
        output: Path | None = None

        try:
            self._clear_failure()
            task.status = TaskStatus.DOWNLOADING_M3U8
            task.started_at = task.started_at or datetime.now().isoformat()
            task.progress.connection_status = "connecting"
            self._set_stage("downloading_m3u8", "正在获取 m3u8 清单")

            concurrency = min(256, max(1, int(task.concurrency or settings.default_concurrency or 12)))
            task.concurrency = concurrency
            headers = self._headers(task.url)
            async with _create_hls_client(concurrency) as client:
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
                    if parsed.get("external_audio"):
                        if is_live:
                            raise UnsupportedPlaylistError(
                                "直播流暂不支持视频与音频分离的清单"
                            )
                        self._set_stage("parsing", "检测到独立 HLS 音轨，正在使用兼容合并引擎")
                        await DashDownloader(
                            task,
                            on_progress=self.on_progress,
                            on_log=self.on_log,
                            source_label="HLS 独立音轨",
                        ).run()
                        return
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
                    (task_dir / "playlist.m3u8").write_text(parsed["content"], encoding="utf-8")

                if is_live:
                    task.engine_state["live"] = True
                    if self._subtitle_tracks:
                        self._log("[subtitles] 直播录制暂不保存外部字幕")
                    if parsed is None:
                        recovered: list[dict] = []
                        total_duration = self._restore_live_segments(
                            saved_live_state, recovered
                        )
                        if not recovered:
                            raise RuntimeError("直播源已不可用，且没有可合并的已录制分片")
                        self._compact_recorded(recovered)
                        self._save_live_state(recovered, total_duration)
                        segments = recovered
                        task.progress.total_segments = len(segments)
                        task.progress.media_duration = total_duration
                        write_playback_plan(task_dir, segments, total_duration)
                    else:
                        recorded = await self._record_live(
                            client, parsed, headers, saved_live_state
                        )
                        if recorded is None:
                            return
                        segments, total_duration = recorded
                else:
                    # A retried task whose stream has since ended downloads as
                    # plain VOD; drop the stale live marker so the UI stops
                    # presenting it as a recording.
                    task.engine_state.pop("live", None)
                    segments = parsed["segments"]
                    if not segments:
                        raise ValueError("m3u8 中没有分片")
                    total_duration = float(parsed["total_duration"] or 0)

                    task.progress.total_segments = len(segments)
                    self._set_stage("parsing", f"解析完成，共 {len(segments)} 个分片")
                    await self._download_init_maps(client, segments, headers)
                    write_playback_plan(task_dir, segments, total_duration)
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
            )

            task.output_path = str(output)
            task.engine_state["output_is_file"] = True
            task.engine_state.pop("reserved_output_path", None)
            if not await verify_task_checksum(task, output, on_progress=self.on_progress, on_log=self.on_log):
                return
            if not task.engine_state.get("live"):
                # Sidecar subtitles are best-effort: a subtitle CDN failure
                # must never fail a fully merged, verified video.
                await self._download_subtitles(headers)
            task.status = TaskStatus.DONE
            task.finished_at = datetime.now().isoformat()
            task.progress.post_percent = 100.0
            task.progress.connection_status = "idle"
            size_mb = output.stat().st_size / 1048576
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
            task.progress.active_workers = 0
            task.progress.active_slots = 0
            task.progress.active_segment_indexes = []
            self._publish()

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
            async with _create_hls_client(2) as client:
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
            response = await client.get(url, headers=self._headers(url, headers))
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
        if any(segment.get("key") for segment in parsed["segments"]):
            raise RuntimeError("暂不支持加密字幕")
        texts: list[str] = []
        for segment in parsed["segments"]:
            segment_url = segment["url"]

            async def load_segment(segment_url=segment_url):
                response = await client.get(
                    segment_url, headers=self._headers(segment_url, headers)
                )
                response.raise_for_status()
                return response

            segment_response = await self._retry_control_request(
                load_segment,
                stage="verifying",
                url=segment_url,
                label="字幕分片",
            )
            texts.append(segment_response.text)
        return texts

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

    def _load_live_state(self) -> dict | None:
        path = self._task_dir() / LIVE_STATE_FILENAME
        if not path.exists():
            return None
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None
        if not isinstance(payload, dict) or not isinstance(payload.get("segments"), list):
            return None
        return payload

    def _save_live_state(self, recorded: list[dict], total_duration: float) -> None:
        payload = {
            "version": 1,
            "total_duration": float(total_duration or 0),
            "segments": [
                {
                    "index": int(entry["index"]),
                    "url": str(entry.get("url") or ""),
                    "duration": float(entry.get("duration") or 0),
                    "media_sequence": int(entry.get("media_sequence") or 0),
                    "discontinuity": bool(entry.get("discontinuity")),
                    "init_path": str(entry.get("init_path") or ""),
                }
                for entry in recorded
            ],
        }
        destination = self._task_dir() / LIVE_STATE_FILENAME
        temporary = destination.with_name(destination.name + ".tmp")
        try:
            temporary.write_text(
                json.dumps(payload, ensure_ascii=False), encoding="utf-8"
            )
            temporary.replace(destination)
        finally:
            temporary.unlink(missing_ok=True)

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
            init_path = str(item.get("init_path") or "")
            if init_path and not Path(init_path).exists():
                gap = True
                continue
            entry = {
                "index": index,
                "url": str(item.get("url") or ""),
                "duration": float(item.get("duration") or 0),
                "media_sequence": int(item.get("media_sequence") or 0),
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
                    # Windows; copy so the plan stays consistent either way.
                    shutil.copyfile(source, destination)
                    source.unlink(missing_ok=True)
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
    ) -> dict:
        async def load_playlist():
            response = await client.get(url, headers=self._headers(url, headers))
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
        next_index = self._compact_recorded(recorded)
        self._purge_orphan_live_segments(next_index)
        if recorded:
            # Compaction may have renamed segment files; persist the new
            # index mapping immediately so a crash before the next batch
            # cannot resurrect the stale layout.
            self._save_live_state(recorded, total_duration)
        recorded_urls = {
            str(entry.get("url") or "") for entry in recorded if entry.get("url")
        }
        last_sequence = max(
            (int(entry.get("media_sequence") or 0) for entry in recorded),
            default=-1,
        )

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
            write_playback_plan(task_dir, recorded, total_duration)
            self._refresh_playback_progress()
        self._set_stage("recording", "直播流录制中，停止录制后自动合并")

        loop = asyncio.get_running_loop()
        last_new_segment = loop.time()
        current = parsed
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
                    segment.get("url") not in recorded_urls for segment in window
                )
            )
            if epoch_reset:
                self._log(
                    "[recording] 直播序号已重置"
                    f"（{last_sequence} → {max(window_sequences)}），继续录制新片段"
                )
            new_batch: list[dict] = []
            capped = False
            projected = total_duration
            for segment in window:
                sequence = int(segment.get("media_sequence") or 0)
                if not epoch_reset and sequence <= last_sequence:
                    continue
                if epoch_reset and segment.get("url") in recorded_urls:
                    continue
                if max_seconds and projected >= max_seconds:
                    # An event playlist can list hours of backlog in a single
                    # window; stop queueing at the cap so the output actually
                    # honors the configured limit.
                    capped = True
                    break
                entry = dict(segment)
                entry["index"] = next_index
                next_index += 1
                if session_boundary or (epoch_reset and not new_batch):
                    entry["discontinuity"] = True
                    session_boundary = False
                projected += float(entry.get("duration") or 0)
                new_batch.append(entry)
            if capped:
                # Only sequences actually queued count as consumed, so a
                # capped batch whose downloads fail can still re-fetch them.
                last_sequence = max(
                    (int(entry.get("media_sequence") or 0) for entry in new_batch),
                    default=last_sequence,
                )
            elif window_sequences:
                if epoch_reset:
                    last_sequence = max(window_sequences)
                else:
                    last_sequence = max(last_sequence, max(window_sequences))

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
                    recorded.extend(kept)
                    recorded_urls.update(
                        str(entry.get("url") or "")
                        for entry in kept
                        if entry.get("url")
                    )
                    total_duration += sum(
                        float(entry.get("duration") or 0) for entry in kept
                    )
                    last_new_segment = loop.time()
                    # Dropped segments burn indexes; renumber so the playback
                    # plan keeps its index == position invariant.
                    next_index = self._compact_recorded(recorded)
                    self._purge_orphan_live_segments(next_index)
                    self.tracker.total = len(recorded)
                    task.progress.media_duration = total_duration
                    write_playback_plan(task_dir, recorded, total_duration)
                    self._save_live_state(recorded, total_duration)
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
                raise RuntimeError("直播清单长时间没有新分片，直播源可能已停止")

            delay = target_duration if new_batch else max(1.0, target_duration / 2)
            await self._live_wait(min(delay, LIVE_MAX_POLL_SECONDS))
            if self._is_canceled():
                raise asyncio.CancelledError
            if self._is_pausing():
                finish_reason = "已停止录制"
                break

            try:
                current = await self._reload_live_playlist(
                    client, playlist_url, headers
                )
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
        self._compact_recorded(final_segments)

        final_duration = sum(
            float(entry.get("duration") or 0) for entry in final_segments
        )
        task.progress.total_segments = len(final_segments)
        task.progress.media_duration = final_duration
        write_playback_plan(task_dir, final_segments, final_duration)
        self._save_live_state(final_segments, final_duration)
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
            for _ in range(min(max(1, concurrency), len(segments)))
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
                raise
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
        except Exception:
            return
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
            temporary.replace(destination)
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
