import asyncio
import re
import shutil
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
from ..models import Task, TaskStatus
from ..utils import sanitize_filename
from .merge import merge_segments
from .errors import as_download_error, diagnose_download_error, format_download_error
from .parser import UnsupportedPlaylistError, parse_m3u8
from .progress import ProgressTracker


MAX_RETRIES = 5
MAX_PLAYLIST_DEPTH = 5
SEG_TIMEOUT = httpx.Timeout(connect=10, read=60, write=30, pool=30)
_CONTENT_RANGE_RE = re.compile(r"^bytes (\d+)-(\d+)/(\d+|\*)$", re.IGNORECASE)


class _BrowserHLSClient:
    def __init__(self, concurrency: int) -> None:
        self._session = CurlAsyncSession(
            max_clients=concurrency + 4,
            impersonate="firefox",
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
        return await self._session.get(url, **kwargs)

    async def download_to_file(
        self,
        url: str,
        destination: Path,
        headers: dict[str, str],
        cancel_check,
    ) -> tuple[Any, int]:
        written = 0
        response = await self._session.get(url, headers=headers, stream=True)
        try:
            with destination.open("wb") as output:
                async for chunk in response.aiter_content():
                    if cancel_check():
                        if response.quit_now:
                            response.quit_now.set()
                        raise asyncio.CancelledError
                    output.write(chunk)
                    written += len(chunk)
        finally:
            if response.astream_task and not response.astream_task.done():
                if response.quit_now:
                    response.quit_now.set()
                await response.aclose()
        return response, written


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

    def _headers(self) -> dict[str, str]:
        headers = {"Accept": "*/*"}
        values = {
            "User-Agent": self.task.user_agent or settings.default_user_agent,
            "Referer": self.task.referer or settings.default_referer,
            "Origin": self.task.origin or settings.default_origin,
            "Cookie": self.task.cookie or settings.default_cookie,
        }
        headers.update({name: value for name, value in values.items() if value})
        return headers

    def _task_dir(self) -> Path:
        return Path(settings.download_dir) / ".tasks" / self.task.id

    def _seg_dir(self) -> Path:
        return self._task_dir() / "segments"

    def _output_path(self) -> Path:
        filename = sanitize_filename(self.task.filename or self.task.title or self.task.id)
        if not filename.lower().endswith(".mp4"):
            filename += ".mp4"
        return _reserve_output_path(Path(settings.download_dir) / filename)

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

    def _clear_failure(self) -> None:
        self.task.error_message = ""
        self.task.error_code = ""
        self.task.error_stage = ""
        self.task.error_url = ""
        self.task.error_hint = ""
        self.task.http_status = 0
        self.task.error_attempt = 0

    def _record_failure(self, exc: BaseException, *, stage: str, url: str = "") -> None:
        details = diagnose_download_error(exc, stage=stage, url=url or self.task.url)
        self.task.error_code = details.code
        self.task.error_stage = details.stage
        self.task.error_url = details.url
        self.task.error_hint = details.hint
        self.task.http_status = details.http_status
        self.task.error_attempt = details.attempt
        self.task.error_message = format_download_error(details)

    def _cleanup_failed_temp(self, task_dir: Path) -> None:
        if settings.keep_temp_files or not task_dir.exists():
            return
        keep = {"download.log", "playlist.m3u8"}
        for child in task_dir.iterdir():
            if child.name in keep:
                continue
            if child.is_dir():
                shutil.rmtree(child, ignore_errors=True)
            else:
                child.unlink(missing_ok=True)

    async def _load_media_playlist(
        self,
        client: Any,
        url: str,
        headers: dict[str, str],
    ) -> dict:
        visited: set[str] = set()
        current_url = url
        for depth in range(MAX_PLAYLIST_DEPTH + 1):
            if current_url in visited:
                raise ValueError(f"主清单存在循环引用: {current_url}")
            visited.add(current_url)
            response = await client.get(current_url, headers=headers)
            response.raise_for_status()
            parsed = parse_m3u8(current_url, response.text)
            if parsed["type"] == "media":
                parsed["content"] = response.text
                return parsed
            if parsed.get("external_audio"):
                raise UnsupportedPlaylistError("暂不支持独立 HLS 音轨")
            if parsed.get("external_subtitles"):
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

            concurrency = max(1, int(task.concurrency or settings.default_concurrency or 8))
            task.concurrency = concurrency
            headers = self._headers()
            async with _create_hls_client(concurrency) as client:
                task.status = TaskStatus.PARSING
                self._set_stage("parsing", "正在解析 HLS 清单")
                parsed = await self._load_media_playlist(client, task.url, headers)
                (task_dir / "playlist.m3u8").write_text(parsed["content"], encoding="utf-8")
                segments = parsed["segments"]
                if not segments:
                    raise ValueError("m3u8 中没有分片")

                task.progress.total_segments = len(segments)
                self._set_stage("parsing", f"解析完成，共 {len(segments)} 个分片")
                await self._download_init_maps(client, segments, headers)

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
                raise RuntimeError(f"{len(self._failed_indexes)} 个分片下载失败，共 {len(segments)} 个")
            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                self._set_stage("canceled", "已取消")
                return

            task.status = TaskStatus.MERGING
            task.progress.connection_status = "idle"
            task.progress.post_percent = 0.0
            self._set_stage("merging", f"正在准备 {len(segments)} 个分片")
            output = self._output_path()
            await merge_segments(
                seg_dir=seg_dir,
                output_path=output,
                segments=segments,
                ffmpeg_path=settings.ffmpeg_path,
                task=task,
                total_duration=parsed["total_duration"],
                on_progress=self.on_progress,
            )

            task.output_path = str(output)
            task.status = TaskStatus.DONE
            task.finished_at = datetime.now().isoformat()
            task.progress.post_percent = 100.0
            task.progress.connection_status = "idle"
            size_mb = output.stat().st_size / 1048576
            self._set_stage("done", f"完成: {output.name} ({size_mb:.1f} MB)")
            if not settings.keep_temp_files:
                shutil.rmtree(task_dir, ignore_errors=True)

        except asyncio.CancelledError:
            task.progress.connection_status = "idle"
            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                task.finished_at = datetime.now().isoformat()
                self._set_stage("canceled", "已取消")
                if output and output.exists() and output.stat().st_size == 0:
                    output.unlink(missing_ok=True)
                if not settings.keep_temp_files:
                    shutil.rmtree(task_dir, ignore_errors=True)
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
            self._cleanup_failed_temp(task_dir)
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
                self._cleanup_failed_temp(task_dir)
        finally:
            task.progress.active_workers = 0
            task.progress.active_slots = 0
            task.progress.active_segment_indexes = []
            self._publish()

    async def _download_init_maps(
        self,
        client: Any,
        segments: list[dict],
        headers: dict[str, str],
    ) -> None:
        map_dir = self._task_dir() / "maps"
        cache: dict[tuple, Path] = {}
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
                destination = map_dir / f"{len(cache):04d}.init"
                if not destination.exists():
                    if key_info:
                        encrypted = destination.with_name(destination.name + ".enc")
                        try:
                            await self._download_resource(
                                client,
                                descriptor["uri"],
                                encrypted,
                                headers,
                                byte_range,
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
                        await self._download_resource(
                            client,
                            descriptor["uri"],
                            destination,
                            headers,
                            byte_range,
                        )
                cache[cache_key] = destination
            segment["init_path"] = str(cache[cache_key])

    async def _download_segments(
        self,
        client: Any,
        segments: list[dict],
        headers: dict[str, str],
        concurrency: int,
    ) -> bool:
        queue: asyncio.Queue[dict] = asyncio.Queue()
        for segment in segments:
            queue.put_nowait(segment)

        self.tracker.start(len(segments))
        self._completed_count = 0
        self._failed_indexes = []
        self._last_segment_error = None
        self.task.progress.failed_segments = 0

        async def worker() -> None:
            while not queue.empty():
                if self._is_canceled() or self._is_pausing():
                    return
                try:
                    segment = queue.get_nowait()
                except asyncio.QueueEmpty:
                    return
                index = segment["index"]
                self.task.progress.active_workers += 1
                self.task.progress.active_slots += 1
                self.task.progress.active_segment_indexes.append(index)
                self._publish()
                try:
                    await self._download_one_segment(client, segment, headers)
                except asyncio.CancelledError:
                    raise
                except Exception as exc:
                    failure = as_download_error(
                        exc,
                        stage="downloading_segments",
                        url=segment["url"],
                        attempt=MAX_RETRIES,
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
                    queue.task_done()
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

        return not self._is_canceled() and not self._is_pausing()

    async def _download_one_segment(
        self,
        client: Any,
        segment: dict,
        headers: dict[str, str],
    ) -> None:
        index = segment["index"]
        destination = self._seg_dir() / f"{index:06d}.seg"
        if destination.exists() and destination.stat().st_size > 0:
            self.tracker.add_completed(destination.stat().st_size)
            self._completed_count += 1
            return

        last_error: Exception | None = None
        for attempt in range(MAX_RETRIES):
            if self._is_canceled() or self._is_pausing():
                return
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
                if self._completed_count % 10 == 0 or self._completed_count == self.task.progress.total_segments:
                    snapshot = self.tracker.snapshot()
                    self._set_stage(
                        "downloading_segments",
                        f"{self._completed_count}/{self.task.progress.total_segments} 分片 "
                        f"{snapshot['speed'] / 1024:.0f} KB/s",
                    )
                return
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                last_error = exc
                self.task.progress.reconnect_count += 1
                self.task.progress.connection_status = "reconnecting"
                destination.unlink(missing_ok=True)
                destination.with_name(destination.name + ".tmp").unlink(missing_ok=True)
                if attempt < MAX_RETRIES - 1:
                    self._log(
                        f"[segment {index}] 第 {attempt + 1}/{MAX_RETRIES} 次失败: {exc}"
                    )
                    await asyncio.sleep(min(2**attempt, 10))
        if last_error is None:
            raise RuntimeError(f"分片 {index} 下载失败")
        raise as_download_error(
            last_error,
            stage="downloading_segments",
            url=segment["url"],
            attempt=MAX_RETRIES,
        ) from last_error

    async def _fetch_key(
        self,
        client: Any,
        url: str,
        headers: dict[str, str],
    ) -> bytes:
        if url not in self._key_cache:
            response = await client.get(url, headers=headers)
            response.raise_for_status()
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
        request_headers = dict(headers)
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
                )
                validate_response(response)
            else:
                async with client.stream("GET", url, headers=request_headers) as response:
                    validate_response(response)
                    with temporary.open("wb") as output:
                        async for chunk in response.aiter_bytes(256 * 1024):
                            if self._is_canceled():
                                raise asyncio.CancelledError
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
