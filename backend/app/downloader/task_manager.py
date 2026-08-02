import asyncio
import binascii
import contextlib
import json
import logging
import os
import shutil
import uuid
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit, urlunsplit

from ..config import settings
from ..database import run_db
from ..models import Task, TaskProgress, TaskStatus, TaskType
from ..naming import suggest_manifest_name
from ..request_context import sanitize_request_contexts, sanitize_request_headers, sanitize_request_replay
from ..credentials import protect_secret, unprotect_secret
from ..checksum import normalize_checksum
from ..sleep_inhibitor import sleep_inhibitor
from ..power_actions import power_action_service
from ..site_profiles import resolve_site_profile
from ..utils import sanitize_filename, stable_request_key
from .hls import HLSDownloader
from .http_file import HTTPDownloader, _resume_resource_identity
from .dash import DashDownloader
from .torrent import TorrentDownloader
from .playback import MIN_START_DURATION, PlaybackError, playback_service
from .engine import task_work_dir, temp_roots


logger = logging.getLogger(__name__)
PROGRESS_EVENT_INTERVAL_SECONDS = 0.25
LOG_QUEUE_CAPACITY = 2000
LOG_MAX_BYTES = 4 * 1024 * 1024
LOG_BACKUP_COUNT = 3


def _decode_request_headers(value: str) -> dict[str, str]:
    try:
        decoded = json.loads(unprotect_secret(value or "") or "{}")
        return sanitize_request_headers(decoded if isinstance(decoded, dict) else {})
    except (OSError, UnicodeError, binascii.Error, TypeError, ValueError, json.JSONDecodeError):
        return {}


def _decode_request_contexts(value: str) -> dict[str, dict]:
    try:
        decoded = json.loads(unprotect_secret(value or "") or "{}")
        return sanitize_request_contexts(decoded if isinstance(decoded, dict) else {})
    except (OSError, UnicodeError, binascii.Error, TypeError, ValueError, json.JSONDecodeError):
        return {}


def _decode_request_body(value: str) -> str:
    try:
        return unprotect_secret(value or "") or ""
    except (OSError, UnicodeError, binascii.Error, TypeError, ValueError):
        return ""


def _decode_cookie(value: str) -> str:
    try:
        return unprotect_secret(value or "") or ""
    except (OSError, UnicodeError, binascii.Error, TypeError, ValueError):
        return ""


def _decode_secret_text(value: str) -> str:
    try:
        return unprotect_secret(value or "") or ""
    except (OSError, UnicodeError, binascii.Error, TypeError, ValueError):
        return ""


def _decode_engine_state(value: str) -> dict:
    try:
        decoded = json.loads(value or "{}")
        return decoded if isinstance(decoded, dict) else {}
    except (TypeError, ValueError, json.JSONDecodeError):
        return {}


def _safe_int(
    value,
    default: int = 0,
    *,
    minimum: int | None = None,
    maximum: int | None = None,
) -> int:
    try:
        result = int(value)
    except (TypeError, ValueError, OverflowError):
        result = int(default)
    if minimum is not None:
        result = max(minimum, result)
    if maximum is not None:
        result = min(maximum, result)
    return result


def _safe_float(value, default: float = 0.0, *, minimum: float | None = None) -> float:
    try:
        result = float(value)
    except (TypeError, ValueError, OverflowError):
        result = float(default)
    if result != result or result in {float("inf"), float("-inf")}:
        result = float(default)
    if minimum is not None:
        result = max(minimum, result)
    return result

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
RESUME_AFTER_UPDATE_KEY = "resume_after_update"


def resolve_task_type(value: TaskType | str, url: str, mime_type: str = "") -> TaskType:
    requested = TaskType(value)
    if requested is not TaskType.AUTO:
        return requested
    lowered = url.lower().split("#", 1)[0].split("?", 1)[0]
    mime = mime_type.lower().split(";", 1)[0].strip()
    if url.lower().startswith("magnet:") or lowered.endswith(".torrent"):
        return TaskType.TORRENT
    if lowered.endswith(".mpd") or mime == "application/dash+xml":
        return TaskType.DASH
    if ".m3u8" in lowered or mime in {
        "application/vnd.apple.mpegurl", "application/x-mpegurl", "application/mpegurl",
    }:
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


def task_output_is_file(task: Task) -> bool:
    if not task.output_path or task.status is not TaskStatus.DONE:
        return False
    cached = task.engine_state.get("output_is_file")
    if cached is not None:
        return bool(cached)
    return task.task_type is not TaskType.TORRENT or task.engine_state.get("stream_path") == task.output_path


def _http_checkpoint_covers_range(task: Task, path: Path, start: int, end: int) -> bool:
    """Prove that a paused sparse file contains the requested durable bytes."""

    try:
        if start < 0 or end < start or not path.is_file():
            return False
        state_path = task_work_dir(task) / "http-resume.json"
        if not state_path.is_file():
            # Sequential files are never preallocated. Their physical length
            # and persisted byte counter form a safe contiguous prefix.
            durable_prefix = min(path.stat().st_size, max(0, int(task.progress.downloaded_bytes)))
            return end < durable_prefix
        saved = json.loads(state_path.read_text(encoding="utf-8"))
        if not isinstance(saved, dict) or not isinstance(saved.get("ranges"), list):
            return False
        saved_identity = str(saved.get("resource_key") or "")
        if not saved_identity:
            saved_identity = _resume_resource_identity(saved.get("url", ""))
        if saved_identity != _resume_resource_identity(task.url):
            return False
        expected_total = int(
            task.engine_state.get("stream_size")
            or task.engine_state.get("total_size")
            or task.progress.total_bytes
            or 0
        )
        if expected_total > 0 and int(saved.get("total") or 0) != expected_total:
            return False
        cursor = start
        for entry in sorted(saved["ranges"], key=lambda item: int(item.get("from", -1))):
            range_start = int(entry["from"])
            current = min(int(entry["current"]), int(entry["to"]) + 1)
            if current <= cursor:
                continue
            if range_start > cursor:
                return False
            cursor = max(cursor, current)
            if cursor > end:
                return True
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError):
        return False
    return False


class TaskManager:
    def __init__(self):
        self.tasks: dict[str, Task] = {}
        self._sem: asyncio.Semaphore | None = None
        self._sem_limit = 0
        self._event_subscribers: list[asyncio.Queue] = []
        self._pending_saves: dict[str, asyncio.Task] = {}
        self._downloaders: dict[str, object] = {}
        self._deleting_task_ids: set[str] = set()
        self._temp_cleanup_lock = asyncio.Lock()
        self._maintenance_task: asyncio.Task | None = None
        self._last_progress_emit: dict[str, float] = {}
        self._log_queue: asyncio.Queue | None = None
        self._log_writer_task: asyncio.Task | None = None

    @staticmethod
    def _queue_schedule_state(now: datetime | None = None) -> tuple[bool, bool]:
        """Return (inside active window, outside because stop boundary passed)."""
        if not settings.queue_auto_start_enabled:
            if not getattr(settings, "queue_auto_stop_enabled", False):
                return True, False
            try:
                stop_hour, stop_minute = (
                    int(value) for value in settings.queue_auto_stop_time.split(":", 1)
                )
                current = now or datetime.now()
                selected = current.weekday() in set(settings.queue_active_days)
                stopped = selected and (current.hour, current.minute) >= (
                    stop_hour,
                    stop_minute,
                )
                return not stopped, stopped
            except (AttributeError, TypeError, ValueError):
                return False, False
        try:
            hour, minute = (int(value) for value in settings.queue_auto_start_time.split(":", 1))
            current = now or datetime.now()
            days = set(int(day) for day in settings.queue_active_days)
            current_time = (current.hour, current.minute)
            start = (hour, minute)
            if not getattr(settings, "queue_auto_stop_enabled", False):
                return current.weekday() in days and current_time >= start, False
            stop = tuple(
                int(value) for value in settings.queue_auto_stop_time.split(":", 1)
            )
            if start < stop:
                active = current.weekday() in days and start <= current_time < stop
                stopped = current.weekday() in days and current_time >= stop
            else:
                previous_day = (current.weekday() - 1) % 7
                active = (
                    current.weekday() in days and current_time >= start
                ) or (previous_day in days and current_time < stop)
                stopped = previous_day in days and stop <= current_time < start
            return active, stopped
        except (AttributeError, TypeError, ValueError):
            return False, False

    @staticmethod
    def _queue_auto_start_due(now: datetime | None = None) -> bool:
        return TaskManager._queue_schedule_state(now)[0]

    @staticmethod
    def _scheduled_time_due(value: str, now: datetime | None = None) -> bool:
        raw = str(value or "").strip()
        if not raw:
            return True
        try:
            target = datetime.fromisoformat(raw)
            if target.tzinfo is None:
                target = target.astimezone()
            current = now or datetime.now().astimezone()
            if current.tzinfo is None:
                current = current.astimezone()
            return current.astimezone(timezone.utc) >= target.astimezone(timezone.utc)
        except (TypeError, ValueError):
            return False

    def _task_scheduled_start_due(self, task: Task, now: datetime | None = None) -> bool:
        return self._scheduled_time_due(
            str(task.engine_state.get("scheduled_start_at") or ""), now
        )

    def _task_scheduled_stop_due(self, task: Task, now: datetime | None = None) -> bool:
        raw = str(task.engine_state.get("scheduled_stop_at") or "")
        return bool(raw) and self._scheduled_time_due(raw, now)

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

    def find_tasks_by_url(self, url: str, *, limit: int = 8) -> list[Task]:
        """Return recent tasks that match the same download URL (IDM-style duplicate check)."""
        target = str(url or '').strip()
        if not target:
            return []
        # Normalize trivial differences that still mean the same resource.
        def normalize(value: str) -> str:
            value = value.strip()
            try:
                from urllib.parse import urlsplit, urlunsplit
                parts = urlsplit(value)
                path = parts.path.rstrip('/') or '/'
                return urlunsplit((parts.scheme.lower(), parts.netloc.lower(), path, parts.query, ''))
            except Exception:
                return value.rstrip('/')
        key = normalize(target)
        matches = [
            task for task in self.tasks.values()
            if normalize(task.url) == key
        ]
        matches.sort(key=lambda task: task.updated_at or task.created_at or '', reverse=True)
        return matches[: max(1, int(limit))]

    def find_expired_request_task(self, url: str, source_page_url: str = "") -> Task | None:
        """Find a stale signed-request task that a browser capture can revive.

        A Core shutdown turns an in-progress task into PAUSED, while a CDN
        signature can rotate before an HTTP probe returns.  Both cases need a
        fresh browser capture just as much as an explicit 403 failure does.
        Deliberately user-paused tasks remain excluded.
        """
        try:
            incoming = urlsplit(str(url or "").strip())
            if incoming.scheme.lower() not in {"http", "https"} or not incoming.netloc:
                return None
            incoming_key = stable_request_key(url)
            incoming_path_key = stable_request_key(url, ignore_host=True)
        except (TypeError, ValueError):
            return None
        page = str(source_page_url or "").split("#", 1)[0]
        candidates: list[Task] = []
        for task in self.tasks.values():
            interrupted_pause = (
                task.status is TaskStatus.PAUSED
                and str(task.engine_state.get("state_reason") or "")
                in {"core_interrupted", "request_refreshed"}
            )
            safe_http_probe = (
                task.task_type is TaskType.HTTP
                and task.status in {
                    TaskStatus.FETCHING_METADATA,
                    TaskStatus.CHECKING,
                    TaskStatus.DOWNLOADING,
                }
                and task.stage in {"", "probing", "checking", "fetching_metadata"}
            )
            if task.status not in {TaskStatus.FAILED, TaskStatus.UNSUPPORTED} and not interrupted_pause and not safe_http_probe:
                continue
            # Never switch a live recorder while its runner is active: it must
            # first preserve/finalize its recorded timeline.  Once a live task
            # has already failed and its runner has exited, HLS restores its
            # persisted live_state.json before polling the refreshed manifest,
            # so accepting a new browser signature safely continues the same
            # recording instead of discarding captured segments.
            failed_live_can_resume = bool(
                task.engine_state.get("live")
                and task.status in {TaskStatus.FAILED, TaskStatus.UNSUPPORTED}
                and not self._has_live_handle(task)
            )
            if task.task_type is TaskType.TORRENT or (
                task.engine_state.get("live") and not failed_live_can_resume
            ):
                continue
            if self._has_live_handle(task) and not safe_http_probe:
                continue
            old_key = stable_request_key(task.url)
            same_resource = old_key == incoming_key
            # Some CDNs rotate edge hostnames along with their signature. Keep
            # that recovery path, but still require every meaningful query
            # parameter (quality, language, asset id, etc.) to match.
            same_page_and_path = bool(
                page
                and task.source_page_url.split("#", 1)[0] == page
                and stable_request_key(task.url, ignore_host=True) == incoming_path_key
            )
            if same_resource or same_page_and_path:
                candidates.append(task)
        candidates.sort(key=lambda item: item.updated_at or item.created_at or "", reverse=True)
        return candidates[0] if candidates else None

    def _get_task(self, task_id: str) -> Task:
        task = self.tasks.get(task_id)
        if task is None:
            raise TaskNotFoundError(f"任务不存在: {task_id}")
        return task

    def _task_is_current(self, task: Task) -> bool:
        """True only while this exact task remains owned by the manager."""
        return task.id not in self._deleting_task_ids and self.tasks.get(task.id) is task

    @staticmethod
    def _has_live_handle(task: Task) -> bool:
        return bool(task.task_handle and not task.task_handle.done())

    def get_available_actions(self, task: Task) -> list[str]:
        live = self._has_live_handle(task)
        actions: list[str] = []
        if task.status in {TaskStatus.QUEUED, TaskStatus.AWAITING_SELECTION} and not live:
            actions.append("start")
        if task.status is TaskStatus.QUEUED:
            position = self.get_queue_position(task)
            queued_count = max(
                len(self._queued_tasks()),
                len([item for item in self.tasks.values() if item.status is TaskStatus.QUEUED]),
            )
            if queued_count > 1:
                if position != 1:
                    actions.extend(["queue_up", "queue_top"])
                if position != queued_count:
                    actions.extend(["queue_down", "queue_bottom"])
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

    @staticmethod
    def _queue_sort_key(item: Task):
        priority = int((item.engine_state or {}).get("queue_priority", 0) or 0)
        return (-priority, item.created_at or "", item.id)

    def _queued_tasks(self) -> list[Task]:
        """Tasks waiting for a download slot or scheduled start."""
        result: list[Task] = []
        for item in self.tasks.values():
            if item.status is not TaskStatus.QUEUED:
                continue
            if (
                self._has_live_handle(item)
                or item.engine_state.get("awaiting_slot")
                or item.engine_state.get("queue_waiting_for_schedule")
            ):
                result.append(item)
        result.sort(key=self._queue_sort_key)
        return result

    def get_queue_position(self, task: Task) -> int:
        if task.status is not TaskStatus.QUEUED:
            return 0
        queued = self._queued_tasks()
        try:
            return queued.index(task) + 1
        except ValueError:
            return 0

    async def _acquire_run_slot(self, task: Task) -> bool:
        """Wait until priority order allows this task under max_concurrent_tasks."""
        task.engine_state["awaiting_slot"] = True
        self._broadcast_queue_updates()
        try:
            while True:
                if task.cancel_event is not None and task.cancel_event.is_set():
                    return False
                limit = max(1, int(settings.max_concurrent_tasks))
                active = len(self._downloaders)
                free = limit - active
                if free > 0:
                    waiting = [
                        item for item in self.tasks.values()
                        if item.engine_state.get("awaiting_slot") and self._has_live_handle(item)
                    ]
                    waiting.sort(key=self._queue_sort_key)
                    if task in waiting[:free]:
                        return True
                await asyncio.sleep(0.12)
        finally:
            task.engine_state.pop("awaiting_slot", None)
            self._broadcast_queue_updates()

    async def reorder_queue(self, task_id: str, direction: str) -> Task:
        task = self._get_task(task_id)
        if task.status is not TaskStatus.QUEUED:
            raise TaskConflictError("只有排队中的任务可以调整顺序")
        direction = str(direction or "").strip().lower()
        if direction not in {"up", "down", "top", "bottom"}:
            raise TaskConflictError("队列方向无效")
        queued = self._queued_tasks()
        if task not in queued:
            queued = sorted(
                (item for item in self.tasks.values() if item.status is TaskStatus.QUEUED),
                key=self._queue_sort_key,
            )
        if task not in queued:
            raise TaskConflictError("任务不在队列中")
        index = queued.index(task)
        if direction == "up" and index > 0:
            queued[index - 1], queued[index] = queued[index], queued[index - 1]
        elif direction == "down" and index < len(queued) - 1:
            queued[index + 1], queued[index] = queued[index], queued[index + 1]
        elif direction == "top" and index > 0:
            queued.pop(index)
            queued.insert(0, task)
        elif direction == "bottom" and index < len(queued) - 1:
            queued.pop(index)
            queued.append(task)
        else:
            return task
        total = len(queued)
        for rank, item in enumerate(queued):
            item.engine_state["queue_priority"] = total - rank
            item.updated_at = datetime.now().isoformat()
            await self._save_db(item)
            self._broadcast_nowait(self._task_event(item))
        self._broadcast_queue_updates()
        return task


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
        request_headers=None,
        request_contexts=None,
        request_method="GET",
        request_body="",
        title="",
        filename="",
        concurrency=0,
        output_dir="",
        checksum="",
        auto_start=False,
        inherit_default_headers=True,
        selected_video="",
        selected_audio="",
        scheduled_start_at="",
        scheduled_stop_at="",
        completion_action="none",
        browser_originated=False,
    ) -> Task:
        task_id = uuid.uuid4().hex
        profile = resolve_site_profile(url)
        safe_headers = {
            **sanitize_request_headers(profile.get("request_headers")),
            **sanitize_request_headers(request_headers),
        }
        safe_method, safe_body = sanitize_request_replay(request_method, request_body, safe_headers)
        resolved_type = resolve_task_type(task_type, url, mime_type)
        # HLS/DASH parsers perform several independent GET requests. A captured
        # POST is safe only as one direct response download, so keep it in the
        # HTTP engine rather than silently dropping the original request body.
        if safe_method == "POST":
            resolved_type = TaskType.HTTP
        if resolved_type in {TaskType.HLS, TaskType.DASH}:
            requested_name = suggest_manifest_name(
                url,
                filename=filename,
                title=title,
                source_page_url=source_page_url,
                fallback=task_id,
            )
        else:
            requested_name = filename or title
        filename = sanitize_filename(requested_name) if requested_name else ""
        now = datetime.now().isoformat()
        inherit_identity_defaults = bool(
            inherit_default_headers and not (source_page_url or request_headers or request_contexts)
        )
        expected_checksum = ""
        checksum_algorithm = ""
        if checksum:
            checksum_algorithm, checksum_digest = normalize_checksum(checksum)
            expected_checksum = f"{checksum_algorithm}:{checksum_digest}"
        task = Task(
            id=task_id,
            url=url,
            task_type=resolved_type,
            source_page_url=source_page_url,
            mime_type=mime_type,
            referer=referer or profile.get("referer", "") or (settings.default_referer if inherit_identity_defaults else ""),
            origin=origin or profile.get("origin", "") or (settings.default_origin if inherit_identity_defaults else ""),
            user_agent=user_agent or profile.get("user_agent", "") or settings.default_user_agent,
            cookie=cookie or (settings.default_cookie if inherit_identity_defaults else ""),
            request_headers=safe_headers,
            request_contexts=sanitize_request_contexts(request_contexts),
            request_method=safe_method,
            request_body=safe_body,
            title=title,
            filename=filename,
            expected_checksum=expected_checksum,
            checksum_algorithm=checksum_algorithm,
            selected_video=str(selected_video or "")[:2048],
            selected_audio=str(selected_audio or "")[:256],
            concurrency=min(64, max(1, int(concurrency or profile.get("concurrency") or settings.default_concurrency or 12))),
            speed_limit_kib=int(profile.get("speed_limit_kib") or 0),
            status=TaskStatus.QUEUED,
            stage="queued",
            last_log="等待开始",
            created_at=now,
            updated_at=now,
            engine_state={
                **({"output_dir": str(Path(output_dir).expanduser().resolve())} if output_dir else {}),
                "temp_dir": str(Path(settings.temp_dir).expanduser().resolve()),
                "inherit_default_headers": inherit_identity_defaults,
                **(
                    {"scheduled_start_at": str(scheduled_start_at)}
                    if scheduled_start_at
                    else {}
                ),
                **(
                    {"scheduled_stop_at": str(scheduled_stop_at)}
                    if scheduled_stop_at
                    else {}
                ),
                "completion_action": str(completion_action or "none"),
                "queue_managed": bool(auto_start),
                "browser_originated": bool(browser_originated),
            },
        )
        async with self._temp_cleanup_lock:
            self.tasks[task_id] = task
        try:
            await run_db(
                "INSERT INTO tasks "
                "(id,task_type,source_page_url,mime_type,title,url,referer,origin,user_agent,cookie,request_headers,request_contexts,request_method,request_body,filename,concurrency,"
                "status,stage,last_log,started_at,finished_at,post_percent,expected_checksum,checksum_algorithm,checksum_actual,checksum_verified,selected_video,selected_audio,engine_state) "
                "VALUES (" + ",".join("?" for _ in range(29)) + ")",
                (
                    task.id,
                    task.task_type.value,
                    protect_secret(task.source_page_url),
                    task.mime_type,
                    task.title,
                    protect_secret(task.url),
                    protect_secret(task.referer),
                    protect_secret(task.origin),
                    task.user_agent,
                    protect_secret(task.cookie),
                    protect_secret(json.dumps(task.request_headers, ensure_ascii=False)),
                    protect_secret(json.dumps(task.request_contexts, ensure_ascii=False)),
                    task.request_method,
                    protect_secret(task.request_body),
                    task.filename,
                    task.concurrency,
                    task.status.value,
                    task.stage,
                    task.last_log,
                    "",
                    "",
                    0,
                    task.expected_checksum,
                    task.checksum_algorithm,
                    "",
                    None,
                    task.selected_video,
                    task.selected_audio,
                    json.dumps(task.engine_state, ensure_ascii=False),
                ),
            )
        except Exception:
            async with self._temp_cleanup_lock:
                if self.tasks.get(task_id) is task:
                    self.tasks.pop(task_id, None)
            raise
        if auto_start:
            if self._queue_auto_start_due() and self._task_scheduled_start_due(task):
                await self.start_task(task_id)
            else:
                task.engine_state["queue_waiting_for_schedule"] = True
                scheduled = str(task.engine_state.get("scheduled_start_at") or "")
                task.last_log = (
                    f"等待任务计划 {scheduled}"
                    if scheduled and not self._task_scheduled_start_due(task)
                    else f"等待定时队列 {settings.queue_auto_start_time}"
                )
                await self._save_db(task)
        self._broadcast_nowait(self._task_event(task, event_type="task_created"))
        return task

    async def start_task(self, task_id: str) -> None:
        task = self._get_task(task_id)
        if task.task_handle and not task.task_handle.done():
            raise TaskConflictError("任务已经在运行")
        if task.status not in {TaskStatus.QUEUED, TaskStatus.PAUSED, TaskStatus.AWAITING_SELECTION}:
            raise TaskConflictError(f"任务状态 {task.status.value} 不能开始")
        if task.status is TaskStatus.AWAITING_SELECTION:
            if task.task_type is not TaskType.TORRENT or not task.engine_state.get("selected_files"):
                raise TaskConflictError("请至少选择一个 BT 文件后再开始下载")
            task.status = TaskStatus.QUEUED
            task.stage = "queued"
            task.last_log = "BT 文件已确认，等待下载队列"

        task.cancel_event = asyncio.Event()
        task.pause_event = asyncio.Event()
        _clear_task_error(task)
        task.checksum_actual = ""
        task.checksum_verified = None
        task.engine_state.pop("completion_action_handled", None)

        async def run_task() -> None:
            try:
                if not await self._acquire_run_slot(task):
                    if task.cancel_event and task.cancel_event.is_set():
                        task.status = TaskStatus.CANCELED
                        task.stage = "canceled"
                        task.last_log = "已取消"
                        task.finished_at = datetime.now().isoformat()
                    return
                try:
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
                    sleep_inhibitor.update(True)
                    try:
                        await downloader.run()
                    finally:
                        self._downloaders.pop(task.id, None)
                        sleep_inhibitor.update(bool(self._downloaders))
                finally:
                    self._broadcast_queue_updates()
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
            except Exception:
                # Engines normally translate protocol failures themselves. A
                # constructor/integration failure before run() must still end
                # in a visible state instead of staying QUEUED forever.
                logger.exception("download runner crashed before finishing task %s", task.id)
                task.status = TaskStatus.FAILED
                task.stage = "failed"
                task.last_log = "下载引擎意外退出，任务已停止；可检查日志后重试"
                task.error_code = "DOWNLOADER_UNEXPECTED_EXIT"
                task.error_stage = "runtime"
                task.error_url = stable_request_key(task.url)
                task.error_hint = "下载引擎未能正常启动或收尾，请重试；重复发生时查看任务日志。"
                task.error_message = (
                    f"[DOWNLOADER_UNEXPECTED_EXIT] {task.last_log}；建议：{task.error_hint}"
                )
                task.progress.connection_status = "error"
                task.finished_at = datetime.now().isoformat()
            finally:
                if task.status is TaskStatus.DONE and task.output_path:
                    from ..windows_attachment import mark_download_from_internet

                    await asyncio.to_thread(
                        mark_download_from_internet,
                        task.output_path,
                        task.url,
                        task.source_page_url,
                    )
                await self._save_db(task)
                await self._cleanup_temp_root_if_all_done()

        task.task_handle = asyncio.create_task(
            run_task(),
            name=f"{task.task_type.value}-{task.id}",
        )
        task.task_handle.add_done_callback(
            lambda handle: self._on_task_finished(task, handle)
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
        # Speculative requests from hls.js must not replace an explicit user seek.
        if not force and task.playback_seek_index is not None:
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
        path, size = self.get_stream_info(task_id)
        if task.status is TaskStatus.DONE:
            return path, size
        if task.task_type is TaskType.HTTP:
            if _http_checkpoint_covers_range(task, path, start, end):
                return path, size
            raise TaskConflictError("目标字节范围尚未下载完成；恢复任务后会自动优先下载")
        if task.task_type is TaskType.TORRENT:
            raise TaskConflictError("BT 任务已暂停，无法确认目标 piece 完整；请先恢复任务")
        return path, size

    async def set_task_speed_limit(self, task_id: str, limit_kib: int) -> None:
        """Apply a per-task download cap immediately; 0 removes it."""
        from .throttle import task_throttles

        task = self._get_task(task_id)
        task.speed_limit_kib = max(0, min(1048576, int(limit_kib or 0)))
        if task.speed_limit_kib > 0:
            task_throttles.bucket(task.id).configure(task.speed_limit_kib)
        else:
            task_throttles.drop(task.id)
        await self._save_db(task)
        self._on_progress(task)

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
        task.engine_state["state_reason"] = "user_pause"
        task.status = TaskStatus.PAUSING
        task.stage = "pausing"
        if task.engine_state.get("live"):
            # Live recording has no resumable middle state: the stop request
            # finalizes and merges everything captured so far.
            task.last_log = "正在停止录制，稍后自动合并已录制内容"
        else:
            task.last_log = "正在等待当前分片完成"
        await self._save_db(task)

    async def prepare_for_update_restart(self) -> int:
        """Persist only running work that a managed update may safely resume."""
        marked = 0
        for task in self.tasks.values():
            running = task.status in ACTIVE_STATUSES or (
                task.status is TaskStatus.QUEUED and self._has_live_handle(task)
            )
            if not running:
                continue
            task.engine_state[RESUME_AFTER_UPDATE_KEY] = True
            task.engine_state["state_reason"] = "update_restart"
            task.last_log = "正在更新，启动新版本后将自动继续下载"
            await self._save_db(task)
            marked += 1
        return marked

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
        # Downloaders publish PAUSED before run_task() finishes its final DB
        # save and queue cleanup. A fast UI click used to observe PAUSED yet
        # receive "任务已经在运行" from start_task(). Join that short tail here
        # so the visible state and the accepted action stay consistent.
        previous = task.task_handle
        if previous and not previous.done():
            try:
                await asyncio.wait_for(asyncio.shield(previous), timeout=10)
            except asyncio.TimeoutError as exc:
                raise TaskConflictError("暂停仍在收尾，请稍后重试") from exc
            except asyncio.CancelledError:
                pass
        await self.start_task(task_id)

    async def refresh_task_request(
        self,
        task_id: str,
        *,
        url: str,
        source_page_url: str | None = None,
        mime_type: str | None = None,
        referer: str | None = None,
        origin: str | None = None,
        user_agent: str | None = None,
        cookie: str | None = None,
        request_headers: dict[str, str] | None = None,
        request_contexts: dict[str, dict] | None = None,
        request_method: str | None = None,
        request_body: str | None = None,
        auto_resume: bool = True,
        browser_originated: bool | None = None,
    ) -> Task:
        """Refresh a signed URL/captured request while preserving resumable data."""
        task = self._get_task(task_id)
        if task.task_type is TaskType.TORRENT or task.status is TaskStatus.DONE:
            raise TaskConflictError("该任务不能更新下载链接")

        # Engines publish FAILED/UNSUPPORTED before their final DB save and
        # handle cleanup. A browser can capture the fresh signed request in
        # that small window; rejecting it with 409 made repeated clicks appear
        # necessary. Join only the terminal tail, never an active download.
        previous = task.task_handle
        if (
            previous
            and not previous.done()
            and task.status in {TaskStatus.FAILED, TaskStatus.UNSUPPORTED, TaskStatus.CANCELED}
        ):
            try:
                await asyncio.wait_for(asyncio.shield(previous), timeout=10)
            except asyncio.TimeoutError as exc:
                raise TaskConflictError("失败任务仍在收尾，请稍后重试") from exc
            except asyncio.CancelledError:
                pass

        was_running = self._has_live_handle(task)
        if was_running:
            if task.engine_state.get("live"):
                raise TaskConflictError("直播录制进行中不能切换主清单；流失效时会先安全合并已录内容")
            if task.status not in {
                TaskStatus.DOWNLOADING_SEGMENTS,
                TaskStatus.DOWNLOADING,
                TaskStatus.FETCHING_METADATA,
                TaskStatus.CHECKING,
            } or task.pause_event is None:
                raise TaskConflictError("当前阶段不能安全更新链接，请稍后重试")
            # A live recorder finalizes on user pause. For request refresh it
            # must instead leave the segment/state directory resumable.
            task.engine_state["state_reason"] = "request_refresh"
            task.pause_event.set()
            task.status = TaskStatus.PAUSING
            task.stage = "pausing"
            task.last_log = "正在暂停并更新下载凭据"
            await self._save_db(task)
            try:
                await asyncio.wait_for(asyncio.shield(task.task_handle), timeout=30)
            except asyncio.TimeoutError as exc:
                raise TaskConflictError("等待下载线程暂停超时，请重试") from exc
            except asyncio.CancelledError:
                pass

        safe_headers = (
            sanitize_request_headers(request_headers)
            if request_headers is not None else task.request_headers
        )
        next_method = request_method if request_method is not None else task.request_method
        next_body = request_body if request_body is not None else task.request_body
        safe_method, safe_body = sanitize_request_replay(next_method, next_body, safe_headers)
        old_url = task.url
        task.url = str(url).strip()
        if source_page_url is not None:
            task.source_page_url = str(source_page_url)
        if mime_type is not None:
            task.mime_type = str(mime_type)
        if referer is not None:
            task.referer = str(referer)
        if origin is not None:
            task.origin = str(origin)
        if user_agent is not None:
            task.user_agent = str(user_agent)
        if cookie is not None:
            task.cookie = str(cookie)
        if request_headers is not None:
            task.request_headers = safe_headers
        if request_contexts is not None:
            task.request_contexts = sanitize_request_contexts(request_contexts)
        task.request_method = safe_method
        task.request_body = safe_body
        task.engine_state.pop("previous_request_url", None)
        if browser_originated is not None:
            task.engine_state["browser_originated"] = bool(browser_originated)
        task.engine_state["previous_request_key"] = stable_request_key(old_url, ignore_host=True)
        task.engine_state["state_reason"] = "request_refreshed"
        task.status = TaskStatus.PAUSED
        task.stage = "paused"
        task.last_log = "下载链接和请求凭据已更新，已有进度已保留"
        task.finished_at = ""
        task.progress.connection_status = "idle"
        task.progress.active_workers = 0
        task.progress.active_slots = 0
        _clear_task_error(task)
        await self._save_db(task)
        if auto_resume:
            await self.start_task(task.id)
        return task

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
        if len(self._deleting_task_ids) >= 1024:
            self._deleting_task_ids.pop()
        self._deleting_task_ids.add(task_id)
        self._last_progress_emit.pop(task_id, None)
        was_complete = task.status is TaskStatus.DONE
        try:
            if task.task_handle and not task.task_handle.done():
                await self.cancel_task(task_id)
            playback_service.close_task(task_id)
            from .throttle import task_throttles

            task_throttles.drop(task_id)
            if delete_files or not was_complete:
                await self._delete_task_outputs(task)
            pending = self._pending_saves.pop(task_id, None)
            if pending and not pending.done():
                pending.cancel()
            await run_db("DELETE FROM tasks WHERE id=?", (task_id,))
            self.tasks.pop(task_id, None)
            task_dir = task_work_dir(task)
            if task_dir.exists():
                await asyncio.to_thread(shutil.rmtree, task_dir, True)
            self._broadcast_nowait({"type": "task_deleted", "task_id": task_id})
            await self._cleanup_temp_root_if_all_done()
        except Exception:
            # The task remains managed if deletion did not complete, so permit
            # normal updates after surfacing the failure to the caller.
            self._deleting_task_ids.discard(task_id)
            raise

    async def _delete_task_outputs(self, task: Task) -> None:
        download_root = Path(task.engine_state.get("output_dir") or settings.download_dir).resolve()
        candidates = {
            str(task.output_path or ""),
            str(task.engine_state.get("reserved_output_path", "") or ""),
            str(task.engine_state.get("stream_path", "") or ""),
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
        task_dir = task_work_dir(task)
        if not task_dir.exists():
            return
        cleanup = None
        if task.status in {TaskStatus.DONE, TaskStatus.CANCELED}:
            def cleanup() -> None:
                shutil.rmtree(task_dir, ignore_errors=True)
        elif task.status in {TaskStatus.FAILED, TaskStatus.UNSUPPORTED}:
            if task.engine_state.get("live"):
                # A failed live recording's temp dir holds the only copy of
                # the captured segments; retry re-finalizes them (same guard
                # as HLSDownloader._cleanup_failed_temp).
                return
            def cleanup() -> None:
                self._trim_failed_task_dir(task_dir)
        if cleanup is not None:
            await asyncio.to_thread(
                playback_service.cleanup_if_inactive,
                task.id,
                cleanup,
            )

    @staticmethod
    def _trim_failed_task_dir(task_dir: Path) -> None:
        resumable = (
            (
                (task_dir / "http-resume.json").is_file()
                and (task_dir / "payload.downloading").is_file()
            )
            or (
                (task_dir / "vod_segments.json").is_file()
                and any((task_dir / "segments").glob("*.seg"))
            )
            or (
                (task_dir / "dash_vod_segments.json").is_file()
                and (
                    any((task_dir / "segments").glob("*.seg"))
                    or any((task_dir / "a").glob("*.m4s"))
                )
            )
        )
        if resumable:
            return
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
            if self.tasks and any(
                task.status not in {TaskStatus.DONE, TaskStatus.CANCELED}
                for task in self.tasks.values()
            ):
                return

            for temp_root in temp_roots():
                if temp_root.name != ".tasks":
                    logger.error("refusing to clean unexpected temp path: %s", temp_root)
                    continue
                if temp_root.exists():
                    await asyncio.to_thread(
                        playback_service.cleanup_if_no_active,
                        set(self.tasks),
                        lambda root=temp_root: shutil.rmtree(root, ignore_errors=True),
                    )

    async def cleanup_orphan_temp_dirs(self) -> None:
        if settings.keep_temp_files:
            return
        for base in temp_roots():
            if not base.exists():
                continue
            for child in base.iterdir():
                if not child.is_dir():
                    continue
                task = self.tasks.get(child.name)
                if task is None:
                    await asyncio.to_thread(shutil.rmtree, child, True)
                elif task.status in {TaskStatus.DONE, TaskStatus.CANCELED}:
                    await self._cleanup_task_temp(task)
        await self._cleanup_temp_root_if_all_done()

    async def load_from_db(self) -> None:
        rows = await run_db("SELECT * FROM tasks ORDER BY created_at ASC")
        interrupted: list[Task] = []
        resume_after_update: list[Task] = []
        scheduled_queue: list[Task] = []
        secret_migrations: list[Task] = []
        for row in rows:
            engine_state = _decode_engine_state(_row_value(row, "engine_state", "{}") or "{}")
            try:
                stored_status = TaskStatus(_row_value(row, "status", TaskStatus.QUEUED.value))
            except (TypeError, ValueError):
                stored_status = TaskStatus.PAUSED
                engine_state["state_reason"] = "database_state_recovered"
            status = stored_status
            stage = _row_value(row, "stage", "")
            last_log = _row_value(row, "last_log", "")
            scheduled = bool(
                stored_status is TaskStatus.QUEUED
                and engine_state.get("queue_waiting_for_schedule")
            )
            if stored_status in ACTIVE_STATUSES or (
                stored_status is TaskStatus.QUEUED and not scheduled
            ):
                status = TaskStatus.PAUSED
                stage = "interrupted"
                last_log = "上次运行中断，可点击恢复"

            progress = TaskProgress(
                total_segments=_safe_int(_row_value(row, "total_segments", 0), minimum=0),
                completed_segments=_safe_int(_row_value(row, "completed_segments", 0), minimum=0),
                failed_segments=_safe_int(_row_value(row, "failed_segments", 0), minimum=0),
                downloaded_bytes=_safe_int(_row_value(row, "downloaded_bytes", 0), minimum=0),
                total_bytes=_safe_int(_row_value(row, "total_bytes", 0), minimum=0),
                speed_bytes_per_sec=_safe_float(_row_value(row, "speed_bytes_per_sec", 0), minimum=0),
                eta_seconds=_safe_float(_row_value(row, "eta_seconds", 0), minimum=0),
                post_percent=_safe_float(_row_value(row, "post_percent", 0), minimum=0),
                playable_segments=_safe_int(_row_value(row, "playable_segments", 0), minimum=0),
                playable_duration=_safe_float(_row_value(row, "playable_duration", 0), minimum=0),
                media_duration=_safe_float(_row_value(row, "media_duration", 0), minimum=0),
                progress_percent=_safe_float(_row_value(row, "progress_percent", 0), minimum=0),
                uploaded_bytes=_safe_int(_row_value(row, "uploaded_bytes", 0), minimum=0),
                upload_speed_bytes_per_sec=_safe_float(
                    _row_value(row, "upload_speed_bytes_per_sec", 0), minimum=0
                ),
                peer_count=_safe_int(_row_value(row, "peer_count", 0), minimum=0),
                seed_count=_safe_int(_row_value(row, "seed_count", 0), minimum=0),
                connection_status="idle",
            )
            request_headers = _decode_request_headers(_row_value(row, "request_headers", "") or "")
            request_method, request_body = sanitize_request_replay(
                _row_value(row, "request_method", "GET") or "GET",
                _decode_request_body(_row_value(row, "request_body", "") or ""),
                request_headers,
            )
            raw_url = _row_value(row, "url", "") or ""
            raw_source_page_url = _row_value(row, "source_page_url", "") or ""
            raw_referer = _row_value(row, "referer", "") or ""
            raw_origin = _row_value(row, "origin", "") or ""
            raw_error_url = _row_value(row, "error_url", "") or ""
            task_url = _decode_secret_text(raw_url)
            try:
                task_type = TaskType(
                    _row_value(row, "task_type", TaskType.HLS.value) or TaskType.HLS.value
                )
            except (TypeError, ValueError):
                task_type = resolve_task_type(
                    TaskType.AUTO,
                    task_url,
                    _row_value(row, "mime_type", "") or "",
                )
                engine_state["state_reason"] = "database_state_recovered"
            task = Task(
                id=row["id"],
                url=task_url,
                task_type=task_type,
                source_page_url=_decode_secret_text(raw_source_page_url),
                mime_type=_row_value(row, "mime_type", "") or "",
                referer=_decode_secret_text(raw_referer),
                origin=_decode_secret_text(raw_origin),
                user_agent=_row_value(row, "user_agent", "") or "",
                cookie=_decode_cookie(_row_value(row, "cookie", "") or ""),
                request_headers=request_headers,
                request_contexts=_decode_request_contexts(_row_value(row, "request_contexts", "") or ""),
                request_method=request_method,
                request_body=request_body,
                title=_row_value(row, "title", "") or "",
                filename=_row_value(row, "filename", "") or "",
                concurrency=_safe_int(
                    _row_value(row, "concurrency", 0) or settings.default_concurrency or 12,
                    12,
                    minimum=1,
                    maximum=256,
                ),
                speed_limit_kib=_safe_int(
                    _row_value(row, "speed_limit_kib", 0), minimum=0, maximum=1048576
                ),
                selected_video=str(_row_value(row, "selected_video", "") or ""),
                selected_audio=str(_row_value(row, "selected_audio", "") or ""),
                status=status,
                progress=progress,
                stage=stage,
                last_log=last_log,
                error_message=_row_value(row, "error_message", "") or "",
                error_code=_row_value(row, "error_code", "") or "",
                error_stage=_row_value(row, "error_stage", "") or "",
                error_url=_decode_secret_text(raw_error_url),
                error_hint=_row_value(row, "error_hint", "") or "",
                http_status=_safe_int(_row_value(row, "http_status", 0), minimum=0, maximum=999),
                error_attempt=_safe_int(_row_value(row, "error_attempt", 0), minimum=0),
                expected_checksum=_row_value(row, "expected_checksum", "") or "",
                checksum_algorithm=_row_value(row, "checksum_algorithm", "") or "",
                checksum_actual=_row_value(row, "checksum_actual", "") or "",
                checksum_verified=(None if _row_value(row, "checksum_verified", None) is None else bool(_row_value(row, "checksum_verified", 0))),
                output_path=_row_value(row, "output_path", "") or "",
                created_at=_row_value(row, "created_at", "") or "",
                updated_at=_row_value(row, "updated_at", "") or "",
                started_at=_row_value(row, "started_at", "") or "",
                finished_at=_row_value(row, "finished_at", "") or "",
                engine_state=engine_state,
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
            if os.name == "nt" and any(
                value and not str(value).startswith("dpapi:")
                for value in (raw_url, raw_source_page_url, raw_referer, raw_origin, raw_error_url)
            ):
                secret_migrations.append(task)
            if status is not stored_status:
                task.engine_state["state_reason"] = "core_interrupted"
                interrupted.append(task)
            if scheduled:
                scheduled_queue.append(task)
            if task.engine_state.pop(RESUME_AFTER_UPDATE_KEY, False):
                task.status = TaskStatus.PAUSED
                task.stage = "queued"
                task.last_log = "更新完成，正在自动继续下载"
                task.engine_state["state_reason"] = "update_restart"
                resume_after_update.append(task)
        for task in interrupted:
            await self._save_db(task)
        interrupted_ids = {task.id for task in interrupted}
        for task in secret_migrations:
            if task.id not in interrupted_ids:
                await self._save_db(task)
        if self._queue_auto_start_due():
            for task in scheduled_queue:
                task.engine_state.pop("queue_waiting_for_schedule", None)
                task.last_log = "定时队列已开始"
                await self._save_db(task)
                try:
                    await self.start_task(task.id)
                except Exception:
                    task.engine_state["queue_waiting_for_schedule"] = True
                    task.last_log = "定时队列启动失败，将自动重试"
                    await self._save_db(task)
                    logger.exception("scheduled task %s failed to start", task.id)
        for task in resume_after_update:
            # Save the cleared marker before starting.  If startup itself is
            # interrupted, the persisted task remains resumable rather than
            # repeatedly carrying a stale update marker.
            await self._save_db(task)
            try:
                await self.start_task(task.id)
            except TaskConflictError as exc:
                task.stage = "paused"
                task.last_log = f"更新后自动继续失败：{exc}"
                await self._save_db(task)

    async def _write_db(self, task: Task) -> None:
        if not self._task_is_current(task):
            return
        task.updated_at = datetime.now().isoformat()
        try:
            await run_db(
                "UPDATE tasks SET status=?,stage=?,last_log=?,total_segments=?,"
                "completed_segments=?,failed_segments=?,downloaded_bytes=?,total_bytes=?,"
                "speed_bytes_per_sec=?,eta_seconds=?,post_percent=?,error_message=?,"
                "playable_segments=?,playable_duration=?,media_duration=?,"
                "error_code=?,error_stage=?,error_url=?,error_hint=?,http_status=?,"
                "error_attempt=?,expected_checksum=?,checksum_algorithm=?,checksum_actual=?,checksum_verified=?,output_path=?,updated_at=?,started_at=?,finished_at=?,"
                "task_type=?,source_page_url=?,mime_type=?,url=?,referer=?,origin=?,user_agent=?,cookie=?,request_headers=?,request_contexts=?,request_method=?,request_body=?,progress_percent=?,uploaded_bytes=?,"
                "upload_speed_bytes_per_sec=?,peer_count=?,seed_count=?,speed_limit_kib=?,engine_state=? WHERE id=?",
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
                    protect_secret(task.error_url),
                    task.error_hint,
                    task.http_status,
                    task.error_attempt,
                    task.expected_checksum,
                    task.checksum_algorithm,
                    task.checksum_actual,
                    None if task.checksum_verified is None else int(task.checksum_verified),
                    task.output_path,
                    task.updated_at,
                    task.started_at or "",
                    task.finished_at or "",
                    task.task_type.value,
                    protect_secret(task.source_page_url),
                    task.mime_type,
                    protect_secret(task.url),
                    protect_secret(task.referer),
                    protect_secret(task.origin),
                    task.user_agent,
                    protect_secret(task.cookie),
                    protect_secret(json.dumps(task.request_headers, ensure_ascii=False)),
                    protect_secret(json.dumps(task.request_contexts, ensure_ascii=False)),
                    task.request_method,
                    protect_secret(task.request_body),
                    task.progress.progress_percent,
                    task.progress.uploaded_bytes,
                    task.progress.upload_speed_bytes_per_sec,
                    task.progress.peer_count,
                    task.progress.seed_count,
                    task.speed_limit_kib,
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
        if not self._task_is_current(task):
            return
        await self._write_db(task)
        self._broadcast_nowait(self._task_event(task))

    def _schedule_save(self, task: Task) -> None:
        if not self._task_is_current(task):
            return
        pending = self._pending_saves.get(task.id)
        if pending and not pending.done():
            return

        async def delayed_save() -> None:
            try:
                await asyncio.sleep(1)
                if self._task_is_current(task):
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
        pending_saves = [save for save in self._pending_saves.values() if not save.done()]
        self._pending_saves.clear()
        for save in pending_saves:
            save.cancel()
        await asyncio.gather(*pending_saves, return_exceptions=True)
        for task in self.tasks.values():
            await self._write_db(task)
        if self._log_writer_task is not None and not self._log_writer_task.done():
            if self._log_queue is not None:
                await self._log_queue.put(None)
            await self._log_writer_task
        self._log_writer_task = None
        self._log_queue = None
        sleep_inhibitor.close()
        power_action_service.close()

    async def _maintain_scheduled_tasks(self, now: datetime | None = None) -> None:
        current = now or datetime.now()
        queue_active, queue_should_stop = self._queue_schedule_state(current)
        if queue_active:
            for task in list(self.tasks.values()):
                if not task.engine_state.get("queue_waiting_for_schedule"):
                    continue
                if not self._task_scheduled_start_due(task, current):
                    continue
                if task.status not in {TaskStatus.QUEUED, TaskStatus.PAUSED} or self._has_live_handle(task):
                    continue
                task.engine_state.pop("queue_waiting_for_schedule", None)
                task.engine_state.pop("queue_window_stopped", None)
                task.last_log = "定时队列已开始"
                await self._save_db(task)
                try:
                    await self.start_task(task.id)
                except Exception:
                    task.engine_state["queue_waiting_for_schedule"] = True
                    task.last_log = "定时队列启动失败，将自动重试"
                    await self._save_db(task)
                    logger.exception("scheduled task %s failed to start", task.id)

        if queue_should_stop:
            for task in list(self.tasks.values()):
                if not task.engine_state.get("queue_managed"):
                    continue
                if task.engine_state.get("queue_window_stopped"):
                    continue
                if task.status in {
                    TaskStatus.DONE,
                    TaskStatus.FAILED,
                    TaskStatus.CANCELED,
                    TaskStatus.UNSUPPORTED,
                }:
                    continue
                task.engine_state["queue_window_stopped"] = True
                task.engine_state["queue_waiting_for_schedule"] = True
                task.last_log = f"定时队列已在 {settings.queue_auto_stop_time} 停止"
                await self._save_db(task)
                try:
                    if task.status in {
                        TaskStatus.CHECKING,
                        TaskStatus.DOWNLOADING,
                        TaskStatus.DOWNLOADING_M3U8,
                        TaskStatus.PARSING,
                        TaskStatus.DOWNLOADING_SEGMENTS,
                        TaskStatus.MERGING,
                        TaskStatus.REMUXING,
                    }:
                        await self.pause_task(task.id)
                except (TaskConflictError, TaskNotFoundError):
                    logger.exception("queue task %s failed to stop", task.id)

        for task in list(self.tasks.values()):
            if task.engine_state.get("scheduled_stop_handled"):
                continue
            if not self._task_scheduled_stop_due(task, current):
                continue
            task.engine_state["scheduled_stop_handled"] = True
            task.last_log = "已到任务计划停止时间"
            await self._save_db(task)
            try:
                if task.status is TaskStatus.QUEUED:
                    await self.cancel_task(task.id)
                elif task.status in {
                    TaskStatus.CHECKING,
                    TaskStatus.DOWNLOADING,
                    TaskStatus.DOWNLOADING_M3U8,
                    TaskStatus.PARSING,
                    TaskStatus.DOWNLOADING_SEGMENTS,
                    TaskStatus.MERGING,
                    TaskStatus.REMUXING,
                }:
                    await self.pause_task(task.id)
            except (TaskConflictError, TaskNotFoundError):
                logger.exception("scheduled task %s failed to stop", task.id)

    def start_maintenance(self) -> None:
        if self._maintenance_task and not self._maintenance_task.done():
            return

        async def maintain() -> None:
            while True:
                await asyncio.sleep(5)
                try:
                    await self._maintain_scheduled_tasks()
                    for task_id in playback_service.expire():
                        task = self.tasks.get(task_id)
                        if task is not None:
                            await self._cleanup_task_temp(task)
                    await self._cleanup_temp_root_if_all_done()
                except asyncio.CancelledError:
                    raise
                except Exception:
                    # A transient database/filesystem error must not silently
                    # kill scheduling and playback cleanup for the process.
                    logger.exception("task maintenance iteration failed")

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
            "download_dir": str(task.engine_state.get("output_dir") or settings.download_dir),
            "concurrency": task.concurrency,
            "speed_limit_kib": task.speed_limit_kib,
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
            "is_live": bool(task.engine_state.get("live")),
            "state_reason": str(task.engine_state.get("state_reason", "")),
            "error_message": task.error_message,
            "error_code": task.error_code,
            "error_stage": task.error_stage,
            "error_url": task.error_url,
            "error_hint": task.error_hint,
            "http_status": task.http_status,
            "error_attempt": task.error_attempt,
            "expected_checksum": task.expected_checksum,
            "checksum_algorithm": task.checksum_algorithm,
            "checksum_actual": task.checksum_actual,
            "checksum_verified": task.checksum_verified,
            "output_path": task.output_path,
            "output_is_file": task_output_is_file(task),
            "created_at": task.created_at,
            "updated_at": task.updated_at,
            "started_at": task.started_at,
            "finished_at": task.finished_at,
            "available_actions": self.get_available_actions(task),
            "queue_position": self.get_queue_position(task),
            "scheduled_start_at": str(task.engine_state.get("scheduled_start_at") or ""),
            "scheduled_stop_at": str(task.engine_state.get("scheduled_stop_at") or ""),
            "completion_action": str(task.engine_state.get("completion_action") or "none"),
        }

    def _on_log_write(self, task_id: str, message: str) -> None:
        if task_id in self._deleting_task_ids:
            return
        message = str(message or "")[:16 * 1024]
        self._broadcast_nowait(
            {"type": "task_log", "task_id": task_id, "message": message}
        )
        try:
            asyncio.get_running_loop()
            self._ensure_log_writer()
            if self._log_queue is None:
                return
            try:
                self._log_queue.put_nowait((task_id, message))
            except asyncio.QueueFull:
                with contextlib.suppress(asyncio.QueueEmpty):
                    self._log_queue.get_nowait()
                self._log_queue.put_nowait((task_id, "[log] 日志写入队列已满，已丢弃最旧记录"))
        except RuntimeError:
            self._write_log_batch([(task_id, message)])
        except Exception as exc:
            logger.warning("log write failed for task %s: %s", task_id, exc)

    def _ensure_log_writer(self) -> None:
        if self._log_writer_task is not None and not self._log_writer_task.done():
            return
        self._log_queue = asyncio.Queue(maxsize=LOG_QUEUE_CAPACITY)

        async def writer() -> None:
            assert self._log_queue is not None
            while True:
                item = await self._log_queue.get()
                if item is None:
                    return
                batch = [item]
                while len(batch) < 100:
                    try:
                        extra = self._log_queue.get_nowait()
                    except asyncio.QueueEmpty:
                        break
                    if extra is None:
                        await asyncio.to_thread(self._write_log_batch, batch)
                        return
                    batch.append(extra)
                await asyncio.to_thread(self._write_log_batch, batch)

        self._log_writer_task = asyncio.create_task(writer(), name="task-log-writer")

    def _write_log_batch(self, batch: list[tuple[str, str]]) -> None:
        grouped: dict[str, list[str]] = {}
        for task_id, message in batch:
            if task_id in self._deleting_task_ids:
                continue
            grouped.setdefault(task_id, []).append(message)
        for task_id, messages in grouped.items():
            try:
                task = self.tasks.get(task_id)
                log_dir = task_work_dir(task or task_id)
                log_dir.mkdir(parents=True, exist_ok=True)
                log_path = log_dir / "download.log"
                payload = "".join(message + "\n" for message in messages)
                if log_path.exists() and log_path.stat().st_size + len(payload.encode("utf-8")) > LOG_MAX_BYTES:
                    oldest = log_path.with_name(f"{log_path.name}.{LOG_BACKUP_COUNT}")
                    oldest.unlink(missing_ok=True)
                    for index in range(LOG_BACKUP_COUNT - 1, 0, -1):
                        source = log_path.with_name(f"{log_path.name}.{index}")
                        if source.exists():
                            source.replace(log_path.with_name(f"{log_path.name}.{index + 1}"))
                    log_path.replace(log_path.with_name(f"{log_path.name}.1"))
                with log_path.open("a", encoding="utf-8") as log_file:
                    log_file.write(payload)
            except Exception as exc:
                logger.warning("log write failed for task %s: %s", task_id, exc)

    def _on_progress(self, task: Task) -> None:
        if not self._task_is_current(task):
            return
        now = asyncio.get_running_loop().time()
        if now - self._last_progress_emit.get(task.id, 0.0) >= PROGRESS_EVENT_INTERVAL_SECONDS:
            self._last_progress_emit[task.id] = now
            self._broadcast_nowait(self._task_event(task))
        self._schedule_save(task)

    def _on_task_finished(
        self,
        task: Task,
        handle: asyncio.Task | None = None,
    ) -> None:
        if not self._task_is_current(task):
            return
        # Download engines are expected to convert ordinary failures into a
        # terminal task state.  This final guard catches an escaped exception
        # or future engine bug, so a finished coroutine can never leave its
        # persisted task at "正在读取文件信息" / "下载中" indefinitely.
        if task.status in ACTIVE_STATUSES | {TaskStatus.QUEUED} and (handle is None or handle.done()):
            reason = "下载线程意外结束，任务已停止以避免一直卡在准备下载"
            if handle is not None and handle.cancelled():
                reason = "下载线程意外中断，任务已停止；可检查链接后重新开始"
            elif handle is not None:
                with contextlib.suppress(asyncio.CancelledError, Exception):
                    if handle.exception() is not None:
                        reason = "下载器发生未处理异常，任务已停止以避免一直卡在准备下载"
            asyncio.create_task(
                self._reconcile_unfinished_task(task, reason),
                name=f"reconcile-{task.id}",
            )
        self._broadcast_nowait(self._task_event(task))
        self._broadcast_queue_updates()
        completion_action = str(task.engine_state.get("completion_action") or "none")
        if (
            task.status is TaskStatus.DONE
            and completion_action != "none"
            and not task.engine_state.get("completion_action_handled")
        ):
            task.engine_state["completion_action_handled"] = True
            asyncio.create_task(self._save_db(task))
            try:
                power_action_service.schedule(
                    task_id=task.id,
                    task_title=task.title or task.filename or task.id,
                    action=completion_action,
                    publish=self._broadcast_nowait,
                )
            except ValueError:
                logger.exception("invalid completion action for task %s", task.id)

    async def _reconcile_unfinished_task(self, task: Task, reason: str) -> None:
        """Persist a terminal status when a completed runner left work active."""
        # run_task has a normal finally that saves first; yield once so this
        # fallback cannot overwrite a legitimate final transition.
        await asyncio.sleep(0)
        if not self._task_is_current(task) or task.status not in ACTIVE_STATUSES | {TaskStatus.QUEUED}:
            return
        if self._has_live_handle(task):
            return
        try:
            parsed = urlsplit(task.url)
            redacted_url = urlunsplit((parsed.scheme, parsed.netloc, parsed.path, "", ""))
        except ValueError:
            redacted_url = ""
        task.status = TaskStatus.FAILED
        task.stage = "failed"
        task.last_log = reason
        task.error_code = "DOWNLOADER_UNEXPECTED_EXIT"
        task.error_stage = "runtime"
        task.error_url = redacted_url
        task.error_hint = "任务运行器已结束但没有进入完成状态。请重新开始；若链接带签名，请从原网页重新识别。"
        task.http_status = 0
        task.error_message = f"[DOWNLOADER_UNEXPECTED_EXIT] {reason}；建议：{task.error_hint}"
        task.progress.connection_status = "error"
        task.finished_at = datetime.now().isoformat()
        await self._save_db(task)
        self._broadcast_nowait(self._task_event(task))
        self._broadcast_queue_updates()


manager = TaskManager()
