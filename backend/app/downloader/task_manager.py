import asyncio
import contextlib
import json
import logging
import shutil
import uuid
from datetime import datetime
from pathlib import Path

from ..config import settings
from ..database import run_db
from ..models import Task, TaskProgress, TaskStatus, TaskType
from ..credentials import protect_secret, unprotect_secret
from ..utils import sanitize_filename
from .hls import HLSDownloader
from .http_file import HTTPDownloader
from .dash import DashDownloader
from .torrent import TorrentDownloader
from .playback import MIN_START_DURATION, PlaybackError, playback_service


logger = logging.getLogger(__name__)

ACTIVE_STATUSES = {
    TaskStatus.FETCHING_METADATA,
    TaskStatus.CHECKING,
    TaskStatus.DOWNLOADING,
    TaskStatus.DOWNLOADING_M3U8,
    TaskStatus.PARSING,
    TaskStatus.DOWNLOADING_SEGMENTS,
    TaskStatus.PAUSING,
    TaskStatus.MERGING,
    TaskStatus.REMUXING,
}
TERMINAL_STATUSES = {
    TaskStatus.DONE,
    TaskStatus.FAILED,
    TaskStatus.CANCELED,
    TaskStatus.UNSUPPORTED,
}
PLAYBACK_STATUSES = {
    TaskStatus.DOWNLOADING,
    TaskStatus.DOWNLOADING_SEGMENTS,
    TaskStatus.PAUSING,
    TaskStatus.PAUSED,
    TaskStatus.MERGING,
    TaskStatus.REMUXING,
}


def resolve_task_type(value: TaskType | str, url: str) -> TaskType:
    requested = TaskType(value)
    if requested is not TaskType.AUTO:
        return requested
    lowered = url.lower().split("#", 1)[0].split("?", 1)[0]
    if url.lower().startswith("magnet:") or lowered.endswith(".torrent"):
        return TaskType.TORRENT
    if lowered.endswith(".mpd"):
        return TaskType.DASH
    if ".m3u8" in lowered:
        return TaskType.HLS
    return TaskType.HTTP


def _clear_task_error(task: Task) -> None:
    task.error_message = ""
    task.error_code = ""
    task.error_stage = ""
    task.error_url = ""
    task.error_hint = ""
    task.http_status = 0
    task.error_attempt = 0


class TaskManagerError(Exception):
    pass


class TaskNotFoundError(TaskManagerError):
    pass


class TaskConflictError(TaskManagerError):
    pass


def _row_value(row, key: str, default=None):
    try:
        return row[key]
    except (KeyError, IndexError):
        return default


class TaskManager:
    def __init__(self):
        self.tasks: dict[str, Task] = {}
        self._sem: asyncio.Semaphore | None = None
        self._sem_limit = 0
        self._event_subscribers: list[asyncio.Queue] = []
        self._sniffed: list[dict] = []
        self._pending_saves: dict[str, asyncio.Task] = {}
        self._downloaders: dict[str, object] = {}
        self._temp_cleanup_lock = asyncio.Lock()
        self._maintenance_task: asyncio.Task | None = None

    def _get_sem(self) -> asyncio.Semaphore:
        limit = max(1, int(settings.max_concurrent_tasks))
        if self._sem is None:
            self._sem = asyncio.Semaphore(limit)
            self._sem_limit = limit
        elif self._sem_limit != limit and not any(
            task.task_handle and not task.task_handle.done() for task in self.tasks.values()
        ):
            self._sem = asyncio.Semaphore(limit)
            self._sem_limit = limit
        return self._sem

    def subscribe(self) -> asyncio.Queue:
        queue = asyncio.Queue(maxsize=200)
        self._event_subscribers.append(queue)
        return queue

    def unsubscribe(self, queue: asyncio.Queue) -> None:
        if queue in self._event_subscribers:
            self._event_subscribers.remove(queue)

    def _broadcast_nowait(self, event: dict) -> None:
        dead = []
        for queue in list(self._event_subscribers):
            try:
                queue.put_nowait(event)
            except asyncio.QueueFull:
                try:
                    queue.get_nowait()
                    queue.put_nowait(event)
                except (asyncio.QueueEmpty, asyncio.QueueFull):
                    dead.append(queue)
        for queue in dead:
            if queue in self._event_subscribers:
                self._event_subscribers.remove(queue)

    async def _broadcast(self, event: dict) -> None:
        self._broadcast_nowait(event)

    def _get_task(self, task_id: str) -> Task:
        task = self.tasks.get(task_id)
        if task is None:
            raise TaskNotFoundError(f"任务不存在: {task_id}")
        return task

    @staticmethod
    def _has_live_handle(task: Task) -> bool:
        return bool(task.task_handle and not task.task_handle.done())

    def get_available_actions(self, task: Task) -> list[str]:
        live = self._has_live_handle(task)
        actions: list[str] = []
        if task.status is TaskStatus.QUEUED and not live:
            actions.append("start")
        if (
            task.status in {
                TaskStatus.DOWNLOADING_SEGMENTS,
                TaskStatus.DOWNLOADING,
                TaskStatus.FETCHING_METADATA,
                TaskStatus.CHECKING,
            }
            and task.pause_event is not None
            and not task.pause_event.is_set()
        ):
            actions.append("pause")
        if task.status is TaskStatus.PAUSED and not live:
            actions.append("resume")
        if task.status not in TERMINAL_STATUSES:
            actions.append("cancel")
        if task.status in {TaskStatus.FAILED, TaskStatus.CANCELED, TaskStatus.UNSUPPORTED} and not live:
            actions.append("retry")
        if self._playback_ready(task):
            actions.append("preview")
        if task.status is TaskStatus.DONE and task.output_path:
            actions.extend(("launch", "open"))
        actions.append("log")
        actions.append("delete")
        if task.status is not TaskStatus.DONE or task.output_path:
            actions.append("delete_files")
        return actions

    @staticmethod
    def _playback_ready(task: Task) -> bool:
        if task.status is TaskStatus.DONE and (
            task.engine_state.get("stream_path") or task.output_path
        ):
            return True
        if (
            task.task_type in {TaskType.HTTP, TaskType.TORRENT}
            and task.status in {TaskStatus.DOWNLOADING, TaskStatus.PAUSING, TaskStatus.PAUSED}
            and task.engine_state.get("stream_path")
        ):
            return True
        progress = task.progress
        return (
            task.status in PLAYBACK_STATUSES
            and progress.playable_segments > 0
            and (
                progress.playable_duration >= MIN_START_DURATION
                or progress.playable_segments == progress.total_segments
            )
        )

    def get_queue_position(self, task: Task) -> int:
        if task.status is not TaskStatus.QUEUED or not self._has_live_handle(task):
            return 0
        queued = sorted(
            (
                item for item in self.tasks.values()
                if item.status is TaskStatus.QUEUED and self._has_live_handle(item)
            ),
            key=lambda item: (item.created_at or "", item.id),
        )
        try:
            return queued.index(task) + 1
        except ValueError:
            return 0

    def _broadcast_queue_updates(self) -> None:
        for task in self.tasks.values():
            if task.status is TaskStatus.QUEUED:
                self._broadcast_nowait(self._task_event(task))

    async def create_task(
        self,
        url,
        task_type=TaskType.AUTO,
        source_page_url="",
        mime_type="",
        referer="",
        origin="",
        user_agent="",
        cookie="",
        title="",
        filename="",
        concurrency=0,
        auto_start=False,
    ) -> Task:
        task_id = str(uuid.uuid4())[:8]
        resolved_type = resolve_task_type(task_type, url)
        requested_name = filename or title or (
            task_id if resolved_type is TaskType.HLS else ""
        )
        filename = sanitize_filename(requested_name) if requested_name else ""
        now = datetime.now().isoformat()
        task = Task(
            id=task_id,
            url=url,
            task_type=resolved_type,
            source_page_url=source_page_url,
            mime_type=mime_type,
            referer=referer or settings.default_referer,
            origin=origin or settings.default_origin,
            user_agent=user_agent or settings.default_user_agent,
            cookie=cookie or settings.default_cookie,
            title=title,
            filename=filename,
            concurrency=concurrency or settings.default_concurrency,
            status=TaskStatus.QUEUED,
            stage="queued",
            last_log="等待开始",
            created_at=now,
            updated_at=now,
        )
        async with self._temp_cleanup_lock:
            self.tasks[task_id] = task
        await run_db(
            "INSERT INTO tasks "
            "(id,task_type,source_page_url,mime_type,title,url,referer,origin,user_agent,cookie,filename,concurrency,"
            "status,stage,last_log,started_at,finished_at,post_percent,engine_state) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (
                task.id,
                task.task_type.value,
                task.source_page_url,
                task.mime_type,
                task.title,
                task.url,
                task.referer,
                task.origin,
                task.user_agent,
                protect_secret(task.cookie),
                task.filename,
                task.concurrency,
                task.status.value,
                task.stage,
                task.last_log,
                "",
                "",
                0,
                "{}",
            ),
        )
        if auto_start:
            await self.start_task(task_id)
        self._broadcast_nowait(self._task_event(task, event_type="task_created"))
        return task

    async def start_task(self, task_id: str) -> None:
        task = self._get_task(task_id)
        if task.task_handle and not task.task_handle.done():
            raise TaskConflictError("任务已经在运行")
        if task.status not in {TaskStatus.QUEUED, TaskStatus.PAUSED}:
            raise TaskConflictError(f"任务状态 {task.status.value} 不能开始")

        task.cancel_event = asyncio.Event()
        task.pause_event = asyncio.Event()
        _clear_task_error(task)

        async def run_task() -> None:
            try:
                async with self._get_sem():
                    if task.cancel_event and task.cancel_event.is_set():
                        return
                    downloader_class = {
                        TaskType.HLS: HLSDownloader,
                        TaskType.HTTP: HTTPDownloader,
                        TaskType.DASH: DashDownloader,
                        TaskType.TORRENT: TorrentDownloader,
                    }[task.task_type]
                    downloader = downloader_class(
                        task,
                        on_progress=self._on_progress,
                        on_log=self._on_log_write,
                    )
                    self._downloaders[task.id] = downloader
                    try:
                        await downloader.run()
                    finally:
                        self._downloaders.pop(task.id, None)
            except asyncio.CancelledError:
                if task.cancel_event and task.cancel_event.is_set():
                    task.status = TaskStatus.CANCELED
                    task.stage = "canceled"
                    task.last_log = "已取消"
                    task.finished_at = datetime.now().isoformat()
                else:
                    task.status = TaskStatus.PAUSED
                    task.stage = "interrupted"
                    task.last_log = "上次运行中断，可点击恢复"
                raise
            finally:
                await self._save_db(task)
                await self._cleanup_temp_root_if_all_done()

        task.task_handle = asyncio.create_task(
            run_task(),
            name=f"{task.task_type.value}-{task.id}",
        )
        task.task_handle.add_done_callback(
            lambda _handle: (
                self._broadcast_nowait(self._task_event(task)),
                self._broadcast_queue_updates(),
            )
        )
        self._broadcast_nowait(self._task_event(task))
        self._broadcast_queue_updates()

    async def request_playback_seek(
        self,
        task_id: str,
        segment_index: int,
        *,
        force: bool = True,
    ) -> None:
        task = self._get_task(task_id)
        if segment_index < 0:
            raise TaskConflictError("播放位置无效")
        if (
            not force
            and task.playback_seek_index is not None
            and segment_index < task.playback_seek_index
        ):
            return
        task.playback_seek_index = int(segment_index)
        downloader = self._downloaders.get(task_id)
        if downloader is not None:
            downloader.request_seek(segment_index)
        self._broadcast_nowait(self._task_event(task))

    def get_stream_info(self, task_id: str) -> tuple[Path, int]:
        task = self._get_task(task_id)
        raw_path = task.engine_state.get("stream_path", "")
        if not raw_path and task.status is TaskStatus.DONE:
            raw_path = task.output_path
        path = Path(raw_path) if raw_path else Path()
        if not raw_path or not path.exists() or not path.is_file():
            raise TaskConflictError("播放文件尚未准备好")
        size = int(
            task.engine_state.get("stream_size")
            or task.engine_state.get("total_size")
            or task.progress.total_bytes
            or path.stat().st_size
        )
        return path, size

    async def wait_for_stream_range(
        self,
        task_id: str,
        start: int,
        end: int,
        timeout: float = 45.0,
    ) -> tuple[Path, int]:
        task = self._get_task(task_id)
        downloader = self._downloaders.get(task_id)
        if downloader is not None and hasattr(downloader, "wait_for_range"):
            path = await downloader.wait_for_range(start, end, timeout=timeout)
            size = int(
                task.engine_state.get("stream_size")
                or task.engine_state.get("total_size")
                or task.progress.total_bytes
            )
            return path, size
        return self.get_stream_info(task_id)

    async def pause_task(self, task_id: str) -> None:
        task = self._get_task(task_id)
        if task.status not in {
            TaskStatus.DOWNLOADING_SEGMENTS,
            TaskStatus.DOWNLOADING,
            TaskStatus.FETCHING_METADATA,
            TaskStatus.CHECKING,
        }:
            raise TaskConflictError("当前阶段不能暂停")
        if task.pause_event is None:
            raise TaskConflictError("任务尚未进入可暂停状态")
        task.pause_event.set()
        task.status = TaskStatus.PAUSING
        task.stage = "pausing"
        task.last_log = "正在等待当前分片完成"
        await self._save_db(task)

    async def select_torrent_files(self, task_id: str, indexes: list[int]) -> None:
        task = self._get_task(task_id)
        if task.task_type is not TaskType.TORRENT:
            raise TaskConflictError("该任务不是 BT 任务")
        files = task.engine_state.get("files", [])
        valid = {int(entry["index"]) for entry in files}
        selected = sorted({int(index) for index in indexes if int(index) in valid})
        if not selected:
            raise TaskConflictError("至少选择一个文件")
        task.engine_state["selected_files"] = selected
        downloader = self._downloaders.get(task_id)
        if isinstance(downloader, TorrentDownloader):
            downloader.select_files(selected)
        await self._save_db(task)

    async def resume_task(self, task_id: str) -> None:
        task = self._get_task(task_id)
        if task.status is not TaskStatus.PAUSED:
            raise TaskConflictError(f"任务状态 {task.status.value} 不能恢复")
        await self.start_task(task_id)

    async def cancel_task(self, task_id: str) -> None:
        task = self._get_task(task_id)
        if task.status in TERMINAL_STATUSES:
            raise TaskConflictError(f"任务状态 {task.status.value} 不能取消")
        if task.cancel_event:
            task.cancel_event.set()
        if task.pause_event:
            task.pause_event.clear()
        handle = task.task_handle
        if handle and not handle.done():
            handle.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await handle

        task.status = TaskStatus.CANCELED
        task.stage = "canceled"
        task.last_log = "用户已取消"
        task.finished_at = datetime.now().isoformat()
        task.progress.connection_status = "idle"
        task.progress.active_workers = 0
        task.progress.active_slots = 0
        await self._save_db(task)
        await self._cleanup_task_temp(task)

    async def retry_task(self, task_id: str) -> None:
        task = self._get_task(task_id)
        if task.task_handle and not task.task_handle.done():
            raise TaskConflictError("任务仍在运行，不能重试")
        if task.status not in {
            TaskStatus.FAILED,
            TaskStatus.CANCELED,
            TaskStatus.UNSUPPORTED,
        }:
            raise TaskConflictError(f"任务状态 {task.status.value} 不能重试")
        task.status = TaskStatus.QUEUED
        task.stage = "queued"
        task.last_log = "正在重试"
        _clear_task_error(task)
        task.output_path = ""
        task.playback_seek_index = None
        task.started_at = ""
        task.finished_at = ""
        task.progress = TaskProgress()
        playback_service.invalidate(task.id)
        await self._save_db(task)
        await self.start_task(task_id)

    async def delete_task(self, task_id: str, *, delete_files: bool = False) -> None:
        task = self._get_task(task_id)
        was_complete = task.status is TaskStatus.DONE
        if task.task_handle and not task.task_handle.done():
            await self.cancel_task(task_id)
        playback_service.close_task(task_id)
        if delete_files or not was_complete:
            await self._delete_task_outputs(task)
        self.tasks.pop(task_id, None)
        pending = self._pending_saves.pop(task_id, None)
        if pending and not pending.done():
            pending.cancel()
        await run_db("DELETE FROM tasks WHERE id=?", (task_id,))
        task_dir = Path(settings.download_dir) / ".tasks" / task_id
        if task_dir.exists():
            await asyncio.to_thread(shutil.rmtree, task_dir, True)
        self._broadcast_nowait({"type": "task_deleted", "task_id": task_id})
        await self._cleanup_temp_root_if_all_done()

    async def _delete_task_outputs(self, task: Task) -> None:
        download_root = Path(settings.download_dir).resolve()
        candidates = {
            str(task.output_path or ""),
            str(task.engine_state.get("reserved_output_path", "") or ""),
        }

        def remove() -> None:
            for raw_path in candidates:
                if not raw_path:
                    continue
                path = Path(raw_path).resolve()
                if path == download_root or download_root not in path.parents:
                    logger.warning("refusing to delete task output outside download directory: %s", path)
                    continue
                if path.is_dir():
                    shutil.rmtree(path, ignore_errors=True)
                else:
                    path.unlink(missing_ok=True)

        await asyncio.to_thread(remove)

    async def release_playback(self, task_id: str, session_id: str) -> bool:
        task = self._get_task(task_id)
        closed = playback_service.close(task_id, session_id)
        if closed:
            await self._cleanup_task_temp(task)
            await self._cleanup_temp_root_if_all_done()
        return closed

    async def _cleanup_task_temp(self, task: Task) -> None:
        if settings.keep_temp_files:
            return
        task_dir = Path(settings.download_dir) / ".tasks" / task.id
        if not task_dir.exists():
            return
        cleanup = None
        if task.status in {TaskStatus.DONE, TaskStatus.CANCELED}:
            cleanup = lambda: shutil.rmtree(task_dir, ignore_errors=True)
        elif task.status in {TaskStatus.FAILED, TaskStatus.UNSUPPORTED}:
            cleanup = lambda: self._trim_failed_task_dir(task_dir)
        if cleanup is not None:
            await asyncio.to_thread(
                playback_service.cleanup_if_inactive,
                task.id,
                cleanup,
            )

    @staticmethod
    def _trim_failed_task_dir(task_dir: Path) -> None:
        keep = {"download.log", "playlist.m3u8"}
        try:
            children = list(task_dir.iterdir())
        except FileNotFoundError:
            return
        for child in children:
            if child.name in keep:
                continue
            if child.is_dir():
                shutil.rmtree(child, ignore_errors=True)
            else:
                child.unlink(missing_ok=True)

    async def _cleanup_temp_root_if_all_done(self) -> None:
        async with self._temp_cleanup_lock:
            if settings.keep_temp_files:
                return
            if self.tasks and any(task.status is not TaskStatus.DONE for task in self.tasks.values()):
                return

            download_dir = Path(settings.download_dir).resolve()
            temp_root = (download_dir / ".tasks").resolve()
            if temp_root.name != ".tasks" or temp_root.parent != download_dir:
                logger.error("refusing to clean unexpected temp path: %s", temp_root)
                return
            if temp_root.exists():
                await asyncio.to_thread(
                    playback_service.cleanup_if_no_active,
                    set(self.tasks),
                    lambda: shutil.rmtree(temp_root, ignore_errors=True),
                )

    async def cleanup_orphan_temp_dirs(self) -> None:
        base = Path(settings.download_dir) / ".tasks"
        if not base.exists() or settings.keep_temp_files:
            return
        for child in base.iterdir():
            if not child.is_dir():
                continue
            task = self.tasks.get(child.name)
            if task is None:
                shutil.rmtree(child, ignore_errors=True)
            elif task.status in {TaskStatus.DONE, TaskStatus.CANCELED}:
                await self._cleanup_task_temp(task)
        await self._cleanup_temp_root_if_all_done()

    async def load_from_db(self) -> None:
        rows = await run_db("SELECT * FROM tasks ORDER BY created_at ASC")
        interrupted: list[Task] = []
        for row in rows:
            stored_status = TaskStatus(_row_value(row, "status", TaskStatus.QUEUED.value))
            status = stored_status
            stage = _row_value(row, "stage", "")
            last_log = _row_value(row, "last_log", "")
            if stored_status in ACTIVE_STATUSES or stored_status is TaskStatus.QUEUED:
                status = TaskStatus.PAUSED
                stage = "interrupted"
                last_log = "上次运行中断，可点击恢复"

            progress = TaskProgress(
                total_segments=int(_row_value(row, "total_segments", 0) or 0),
                completed_segments=int(_row_value(row, "completed_segments", 0) or 0),
                failed_segments=int(_row_value(row, "failed_segments", 0) or 0),
                downloaded_bytes=int(_row_value(row, "downloaded_bytes", 0) or 0),
                total_bytes=int(_row_value(row, "total_bytes", 0) or 0),
                speed_bytes_per_sec=float(_row_value(row, "speed_bytes_per_sec", 0) or 0),
                eta_seconds=float(_row_value(row, "eta_seconds", 0) or 0),
                post_percent=float(_row_value(row, "post_percent", 0) or 0),
                playable_segments=int(_row_value(row, "playable_segments", 0) or 0),
                playable_duration=float(_row_value(row, "playable_duration", 0) or 0),
                media_duration=float(_row_value(row, "media_duration", 0) or 0),
                progress_percent=float(_row_value(row, "progress_percent", 0) or 0),
                uploaded_bytes=int(_row_value(row, "uploaded_bytes", 0) or 0),
                upload_speed_bytes_per_sec=float(_row_value(row, "upload_speed_bytes_per_sec", 0) or 0),
                peer_count=int(_row_value(row, "peer_count", 0) or 0),
                seed_count=int(_row_value(row, "seed_count", 0) or 0),
                connection_status="idle",
            )
            task = Task(
                id=row["id"],
                url=row["url"],
                task_type=TaskType(_row_value(row, "task_type", TaskType.HLS.value) or TaskType.HLS.value),
                source_page_url=_row_value(row, "source_page_url", "") or "",
                mime_type=_row_value(row, "mime_type", "") or "",
                referer=_row_value(row, "referer", "") or "",
                origin=_row_value(row, "origin", "") or "",
                user_agent=_row_value(row, "user_agent", "") or "",
                cookie=unprotect_secret(_row_value(row, "cookie", "") or ""),
                title=_row_value(row, "title", "") or "",
                filename=_row_value(row, "filename", "") or "",
                concurrency=int(_row_value(row, "concurrency", 0) or 0),
                status=status,
                progress=progress,
                stage=stage,
                last_log=last_log,
                error_message=_row_value(row, "error_message", "") or "",
                error_code=_row_value(row, "error_code", "") or "",
                error_stage=_row_value(row, "error_stage", "") or "",
                error_url=_row_value(row, "error_url", "") or "",
                error_hint=_row_value(row, "error_hint", "") or "",
                http_status=int(_row_value(row, "http_status", 0) or 0),
                error_attempt=int(_row_value(row, "error_attempt", 0) or 0),
                output_path=_row_value(row, "output_path", "") or "",
                created_at=_row_value(row, "created_at", "") or "",
                updated_at=_row_value(row, "updated_at", "") or "",
                started_at=_row_value(row, "started_at", "") or "",
                finished_at=_row_value(row, "finished_at", "") or "",
                engine_state=json.loads(_row_value(row, "engine_state", "{}") or "{}"),
            )
            if status in PLAYBACK_STATUSES:
                try:
                    snapshot = playback_service.snapshot(task.id, status.value, task.output_path)
                    task.progress.playable_segments = snapshot.available_segments
                    task.progress.playable_duration = snapshot.available_duration
                    task.progress.media_duration = snapshot.total_duration
                except PlaybackError:
                    task.progress.playable_segments = 0
                    task.progress.playable_duration = 0
            self.tasks[task.id] = task
            if status is not stored_status:
                interrupted.append(task)
        for task in interrupted:
            await self._save_db(task)

    async def _write_db(self, task: Task) -> None:
        task.updated_at = datetime.now().isoformat()
        try:
            await run_db(
                "UPDATE tasks SET status=?,stage=?,last_log=?,total_segments=?,"
                "completed_segments=?,failed_segments=?,downloaded_bytes=?,total_bytes=?,"
                "speed_bytes_per_sec=?,eta_seconds=?,post_percent=?,error_message=?,"
                "playable_segments=?,playable_duration=?,media_duration=?,"
                "error_code=?,error_stage=?,error_url=?,error_hint=?,http_status=?,"
                "error_attempt=?,output_path=?,updated_at=?,started_at=?,finished_at=?,"
                "task_type=?,source_page_url=?,mime_type=?,progress_percent=?,uploaded_bytes=?,"
                "upload_speed_bytes_per_sec=?,peer_count=?,seed_count=?,engine_state=? WHERE id=?",
                (
                    task.status.value,
                    task.stage,
                    task.last_log,
                    task.progress.total_segments,
                    task.progress.completed_segments,
                    task.progress.failed_segments,
                    task.progress.downloaded_bytes,
                    task.progress.total_bytes,
                    task.progress.speed_bytes_per_sec,
                    task.progress.eta_seconds,
                    task.progress.post_percent,
                    task.error_message,
                    task.progress.playable_segments,
                    task.progress.playable_duration,
                    task.progress.media_duration,
                    task.error_code,
                    task.error_stage,
                    task.error_url,
                    task.error_hint,
                    task.http_status,
                    task.error_attempt,
                    task.output_path,
                    task.updated_at,
                    task.started_at or "",
                    task.finished_at or "",
                    task.task_type.value,
                    task.source_page_url,
                    task.mime_type,
                    task.progress.progress_percent,
                    task.progress.uploaded_bytes,
                    task.progress.upload_speed_bytes_per_sec,
                    task.progress.peer_count,
                    task.progress.seed_count,
                    json.dumps(task.engine_state, ensure_ascii=False),
                    task.id,
                ),
            )
        except Exception as exc:
            logger.warning("database save failed for task %s: %s", task.id, exc)

    async def _save_db(self, task: Task) -> None:
        pending = self._pending_saves.pop(task.id, None)
        current = asyncio.current_task()
        if pending and pending is not current and not pending.done():
            pending.cancel()
        await self._write_db(task)
        self._broadcast_nowait(self._task_event(task))

    def _schedule_save(self, task: Task) -> None:
        pending = self._pending_saves.get(task.id)
        if pending and not pending.done():
            return

        async def delayed_save() -> None:
            try:
                await asyncio.sleep(1)
                await self._write_db(task)
            finally:
                self._pending_saves.pop(task.id, None)

        self._pending_saves[task.id] = asyncio.create_task(delayed_save())

    async def shutdown(self) -> None:
        if self._maintenance_task and not self._maintenance_task.done():
            self._maintenance_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._maintenance_task
        handles = [
            task.task_handle
            for task in self.tasks.values()
            if task.task_handle and not task.task_handle.done()
        ]
        for handle in handles:
            handle.cancel()
        await asyncio.gather(*handles, return_exceptions=True)
        for task in self.tasks.values():
            await self._write_db(task)

    def start_maintenance(self) -> None:
        if self._maintenance_task and not self._maintenance_task.done():
            return

        async def maintain() -> None:
            while True:
                await asyncio.sleep(30)
                for task_id in playback_service.expire():
                    task = self.tasks.get(task_id)
                    if task is not None:
                        await self._cleanup_task_temp(task)
                await self._cleanup_temp_root_if_all_done()

        self._maintenance_task = asyncio.create_task(maintain(), name="playback-cleanup")

    def _task_event(self, task: Task, event_type: str = "task_progress") -> dict:
        progress = task.progress
        progress_percent = progress.progress_percent
        if not progress_percent and progress.total_segments:
            progress_percent = min(
                100.0,
                progress.completed_segments * 100 / progress.total_segments,
            )
        return {
            "type": event_type,
            "task_id": task.id,
            "id": task.id,
            "task_type": task.task_type.value,
            "source_page_url": task.source_page_url,
            "mime_type": task.mime_type,
            "title": task.title,
            "url": task.url,
            "referer": task.referer,
            "origin": task.origin,
            "user_agent": task.user_agent,
            "cookie": "",
            "filename": task.filename,
            "concurrency": task.concurrency,
            "status": task.status.value,
            "stage": task.stage,
            "last_log": task.last_log,
            "total_segments": progress.total_segments,
            "completed_segments": progress.completed_segments,
            "failed_segments": progress.failed_segments,
            "downloaded_bytes": progress.downloaded_bytes,
            "total_bytes": progress.total_bytes,
            "speed_bytes_per_sec": progress.speed_bytes_per_sec,
            "eta_seconds": progress.eta_seconds,
            "active_workers": progress.active_workers,
            "max_workers": progress.max_workers,
            "reconnect_count": progress.reconnect_count,
            "connection_status": progress.connection_status,
            "last_worker_error": progress.last_worker_error,
            "post_percent": progress.post_percent,
            "active_slots": progress.active_slots,
            "active_segment_indexes": list(progress.active_segment_indexes),
            "playable_segments": progress.playable_segments,
            "playable_duration": progress.playable_duration,
            "media_duration": progress.media_duration,
            "progress_percent": progress_percent,
            "uploaded_bytes": progress.uploaded_bytes,
            "upload_speed_bytes_per_sec": progress.upload_speed_bytes_per_sec,
            "peer_count": progress.peer_count,
            "seed_count": progress.seed_count,
            "playback_ready": self._playback_ready(task),
            "error_message": task.error_message,
            "error_code": task.error_code,
            "error_stage": task.error_stage,
            "error_url": task.error_url,
            "error_hint": task.error_hint,
            "http_status": task.http_status,
            "error_attempt": task.error_attempt,
            "output_path": task.output_path,
            "output_is_file": bool(task.output_path and Path(task.output_path).is_file()),
            "created_at": task.created_at,
            "updated_at": task.updated_at,
            "started_at": task.started_at,
            "finished_at": task.finished_at,
            "available_actions": self.get_available_actions(task),
            "queue_position": self.get_queue_position(task),
        }

    def _on_log_write(self, task_id: str, message: str) -> None:
        self._broadcast_nowait(
            {"type": "task_log", "task_id": task_id, "message": message}
        )
        try:
            log_dir = Path(settings.download_dir) / ".tasks" / task_id
            log_dir.mkdir(parents=True, exist_ok=True)
            with (log_dir / "download.log").open("a", encoding="utf-8") as log_file:
                log_file.write(message + "\n")
        except Exception as exc:
            logger.warning("log write failed for task %s: %s", task_id, exc)

    def _on_progress(self, task: Task) -> None:
        self._broadcast_nowait(self._task_event(task))
        self._schedule_save(task)


manager = TaskManager()
