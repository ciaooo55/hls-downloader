from __future__ import annotations

import asyncio
import json
import re
import shutil
import time
from datetime import datetime
from pathlib import Path
from urllib.parse import unquote, urlparse

import httpx

from ..config import settings
from ..models import Task, TaskStatus
from ..utils import sanitize_filename
from .engine import SeeklessEngine, task_output_dir
from .errors import diagnose_download_error, format_download_error


MAX_RETRIES = 5
_CONTENT_RANGE_RE = re.compile(r"^bytes (\d+)-(\d+)/(\d+)$", re.IGNORECASE)


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
    if not value:
        return ""
    encoded = re.search(r"filename\*=UTF-8''([^;]+)", value, re.IGNORECASE)
    if encoded:
        return unquote(encoded.group(1).strip())
    plain = re.search(r'filename\s*=\s*(?:"([^"]+)"|([^;]+))', value, re.IGNORECASE)
    if plain:
        return (plain.group(1) or plain.group(2) or "").strip()
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
        self._chunk_size = max(1, int(settings.http_chunk_size_mb)) * 1024 * 1024
        self._part_path: Path | None = None
        self._total_size = 0
        self._sequential = False

    def request_seek(self, value: int) -> None:
        if value >= 0:
            self._priority_chunk = int(value) // self._chunk_size
            if self._priority_queue is not None:
                self._priority_queue.put_nowait((-100, self._priority_chunk))

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
        deadline = time.monotonic() + timeout
        while not required.issubset(self._completed_chunks):
            if time.monotonic() >= deadline:
                raise TimeoutError("目标字节范围尚未下载完成")
            await asyncio.sleep(0.1)
        return self._part_path

    def _headers(self) -> dict[str, str]:
        values = {
            "User-Agent": self.task.user_agent or settings.default_user_agent,
            "Referer": self.task.referer or settings.default_referer,
            "Origin": self.task.origin or settings.default_origin,
            "Cookie": self.task.cookie or settings.default_cookie,
        }
        return {name: value for name, value in values.items() if value}

    def _publish(self) -> None:
        self.on_progress(self.task)

    def _set_stage(self, stage: str, message: str) -> None:
        self.task.stage = stage
        self.task.last_log = message
        self.on_log(self.task.id, f"[{stage}] {message}")
        self._publish()

    def _is_canceled(self) -> bool:
        return bool(self.task.cancel_event and self.task.cancel_event.is_set())

    def _is_pausing(self) -> bool:
        return bool(self.task.pause_event and self.task.pause_event.is_set())

    async def _probe(self, client: httpx.AsyncClient, headers: dict[str, str]) -> dict:
        response = await client.head(self.task.url, headers=headers)
        if response.status_code in {405, 501} or response.status_code >= 500:
            request = client.build_request(
                "GET",
                self.task.url,
                headers={**headers, "Range": "bytes=0-0"},
            )
            response = await client.send(request, stream=True)
        response.raise_for_status()
        try:
            content_range = response.headers.get("content-range", "")
            match = _CONTENT_RANGE_RE.match(content_range)
            total = int(match.group(3)) if match else int(response.headers.get("content-length", 0) or 0)
            return {
                "total": total,
                "ranges": response.status_code == 206 or "bytes" in response.headers.get("accept-ranges", "").lower(),
                "etag": response.headers.get("etag", ""),
                "last_modified": response.headers.get("last-modified", ""),
                "content_type": response.headers.get("content-type", "").split(";", 1)[0],
                "filename": _content_disposition_filename(response.headers.get("content-disposition", "")),
            }
        finally:
            await response.aclose()

    async def run(self) -> None:
        task = self.task
        task_dir = Path(settings.download_dir) / ".tasks" / task.id
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
            async with httpx.AsyncClient(follow_redirects=True, timeout=timeout, limits=limits) as client:
                metadata = await self._probe(client, headers)
                total = int(metadata["total"])
                self._total_size = total
                task.mime_type = task.mime_type or metadata["content_type"]
                task.progress.total_bytes = total
                name = metadata["filename"] or Path(urlparse(task.url).path).name or task.filename or task.id
                task.filename = sanitize_filename(task.filename or name)
                output = _reserve_output_path(task_output_dir(task) / task.filename)
                task.engine_state["reserved_output_path"] = str(output)

                if total <= 0 or not metadata["ranges"]:
                    self._sequential = True
                    await self._download_sequential(client, headers, part_path)
                else:
                    await self._download_ranges(client, headers, part_path, state_path, metadata)

            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                task.finished_at = datetime.now().isoformat()
                self._set_stage("canceled", "已取消")
                return
            if self._is_pausing():
                task.status = TaskStatus.PAUSED
                self._set_stage("paused", "已暂停，可继续下载")
                return
            if not part_path.exists() or part_path.stat().st_size <= 0:
                raise RuntimeError("下载结果为空")
            if task.progress.total_bytes and part_path.stat().st_size != task.progress.total_bytes:
                raise RuntimeError(
                    f"文件长度不匹配，期望 {task.progress.total_bytes}，实际 {part_path.stat().st_size}"
                )
            part_path.replace(output)
            state_path.unlink(missing_ok=True)
            task.output_path = str(output)
            task.engine_state["output_is_file"] = True
            task.engine_state.pop("reserved_output_path", None)
            task.engine_state["stream_path"] = str(output)
            task.engine_state["total_size"] = output.stat().st_size
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
            details = diagnose_download_error(exc, stage=task.stage, url=task.url)
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
        started = time.monotonic()
        async with client.stream("GET", task.url, headers=headers) as response:
            response.raise_for_status()
            with part_path.open("wb") as output:
                async for chunk in response.aiter_bytes(256 * 1024):
                    if self._is_canceled():
                        raise asyncio.CancelledError
                    if self._is_pausing():
                        return
                    output.write(chunk)
                    task.progress.downloaded_bytes += len(chunk)
                    elapsed = max(0.001, time.monotonic() - started)
                    task.progress.speed_bytes_per_sec = task.progress.downloaded_bytes / elapsed
                    if task.progress.total_bytes:
                        task.progress.progress_percent = min(100.0, task.progress.downloaded_bytes * 100 / task.progress.total_bytes)
                    self._publish()
        task.progress.completed_segments = 1

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
        if state_path.exists() and part_path.exists():
            try:
                saved = json.loads(state_path.read_text(encoding="utf-8"))
                if (
                    saved.get("url") == task.url
                    and saved.get("total") == total
                    and saved.get("etag", "") == metadata["etag"]
                    and saved.get("last_modified", "") == metadata["last_modified"]
                ):
                    completed = {int(value) for value in saved.get("completed", []) if int(value) < len(chunks)}
            except (OSError, ValueError, TypeError):
                completed = set()
        if not part_path.exists() or part_path.stat().st_size != total:
            completed.clear()
            with part_path.open("wb") as output:
                output.truncate(total)
        task.progress.total_segments = len(chunks)
        self._completed_chunks = completed
        self._total_size = total
        task.engine_state["stream_path"] = str(part_path)
        task.engine_state["chunk_size"] = chunk_size
        task.engine_state["total_size"] = total
        task.progress.completed_segments = len(completed)
        task.progress.downloaded_bytes = sum(chunks[index][1] - chunks[index][0] + 1 for index in completed)
        task.progress.max_workers = min(task.concurrency, len(chunks))
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
        started = time.monotonic()

        async def save_state() -> None:
            payload = {
                "url": task.url,
                "total": total,
                "etag": metadata["etag"],
                "last_modified": metadata["last_modified"],
                "completed": sorted(completed),
            }
            temporary = state_path.with_suffix(".tmp")
            temporary.write_text(json.dumps(payload), encoding="utf-8")
            temporary.replace(state_path)

        async def worker() -> None:
            while not queue.empty() and not self._is_canceled() and not self._is_pausing():
                try:
                    _, index = queue.get_nowait()
                except asyncio.QueueEmpty:
                    return
                if index in completed or index in self._claimed_chunks or index >= len(chunks):
                    queue.task_done()
                    continue
                self._claimed_chunks.add(index)
                start, end = chunks[index]
                last_error: Exception | None = None
                for attempt in range(1, MAX_RETRIES + 1):
                    if self._is_canceled():
                        raise asyncio.CancelledError
                    try:
                        expected = end - start + 1
                        received = 0
                        async with client.stream(
                            "GET",
                            task.url,
                            headers={**headers, "Range": f"bytes={start}-{end}"},
                        ) as response:
                            response.raise_for_status()
                            match = _CONTENT_RANGE_RE.match(response.headers.get("content-range", ""))
                            if response.status_code != 206 or not match:
                                raise RuntimeError("Range 响应缺少有效 Content-Range")
                            if int(match.group(1)) != start or int(match.group(2)) != end or int(match.group(3)) != total:
                                raise RuntimeError("Range 响应范围与请求不一致")
                            with part_path.open("r+b", buffering=0) as output_file:
                                output_file.seek(start)
                                async for content in response.aiter_bytes(256 * 1024):
                                    received += len(content)
                                    if received > expected:
                                        raise RuntimeError("Range 响应长度超过请求范围")
                                    output_file.write(content)
                        if received != expected:
                            raise RuntimeError(f"Range 长度不匹配，期望 {expected}，实际 {received}")
                        async with state_lock:
                            completed.add(index)
                            self._completed_chunks.add(index)
                            self._claimed_chunks.discard(index)
                            task.engine_state["completed_chunks"] = sorted(completed)
                            task.progress.completed_segments = len(completed)
                            task.progress.downloaded_bytes += expected
                            task.progress.progress_percent = task.progress.downloaded_bytes * 100 / total
                            elapsed = max(0.001, time.monotonic() - started)
                            task.progress.speed_bytes_per_sec = task.progress.downloaded_bytes / elapsed
                            await save_state()
                            self._publish()
                        last_error = None
                        break
                    except Exception as exc:
                        last_error = exc
                        if attempt < MAX_RETRIES:
                            await asyncio.sleep(min(4, attempt))
                if last_error is not None:
                    self._claimed_chunks.discard(index)
                    raise last_error
                queue.task_done()

        workers = [asyncio.create_task(worker()) for _ in range(task.progress.max_workers)]
        results = await asyncio.gather(*workers, return_exceptions=True)
        error = next((result for result in results if isinstance(result, Exception)), None)
        if error:
            raise error
        self._priority_queue = None
