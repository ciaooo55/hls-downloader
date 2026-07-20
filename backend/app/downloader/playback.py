import asyncio
import json
import math
import re
import threading
import time
import uuid
from dataclasses import asdict, dataclass
from pathlib import Path
from urllib.parse import quote

from ..config import settings


PLAN_FILENAME = "playback-plan.json"
MIN_START_DURATION = 6.0
SESSION_TTL_SECONDS = 90.0
_TASK_ID_RE = re.compile(r"^[A-Za-z0-9_-]{1,64}$")


class PlaybackError(Exception):
    pass


class PlaybackNotReadyError(PlaybackError):
    pass


class PlaybackSessionError(PlaybackError):
    pass


@dataclass(frozen=True)
class PlaybackSegment:
    index: int
    duration: float
    discontinuity: bool = False
    init_name: str = ""


@dataclass(frozen=True)
class PlaybackPlan:
    total_duration: float
    target_duration: int
    is_fmp4: bool
    segments: tuple[PlaybackSegment, ...]


@dataclass(frozen=True)
class PlaybackSnapshot:
    ready: bool
    mode: str
    available_segments: int
    total_segments: int
    available_duration: float
    total_duration: float
    complete: bool

    def to_dict(self) -> dict:
        return asdict(self)


@dataclass
class _PlaybackSession:
    task_id: str
    last_seen: float
    requested_index: int | None = None
    requested_time: float = 0.0


def _safe_task_dir(task_id: str) -> Path:
    if not _TASK_ID_RE.fullmatch(task_id):
        raise PlaybackError("无效的任务编号")
    download_dir = Path(settings.download_dir).resolve()
    task_dir = (download_dir / ".tasks" / task_id).resolve()
    if task_dir.parent != (download_dir / ".tasks").resolve():
        raise PlaybackError("无效的任务目录")
    return task_dir


def write_playback_plan(
    task_dir: Path,
    segments: list[dict],
    total_duration: float,
) -> Path:
    safe_segments = []
    for segment in segments:
        init_name = ""
        if segment.get("init_path"):
            init_name = Path(segment["init_path"]).name
        safe_segments.append(
            {
                "index": int(segment["index"]),
                "duration": max(0.001, float(segment.get("duration") or 0)),
                "discontinuity": bool(segment.get("discontinuity")),
                "init_name": init_name,
            }
        )

    target_duration = max(
        1,
        math.ceil(max((segment["duration"] for segment in safe_segments), default=1)),
    )
    payload = {
        "version": 1,
        "total_duration": max(0.0, float(total_duration or 0)),
        "target_duration": target_duration,
        "is_fmp4": any(segment["init_name"] for segment in safe_segments),
        "segments": safe_segments,
    }
    task_dir.mkdir(parents=True, exist_ok=True)
    destination = task_dir / PLAN_FILENAME
    temporary = destination.with_name(destination.name + ".tmp")
    try:
        temporary.write_text(
            json.dumps(payload, ensure_ascii=False, separators=(",", ":")),
            encoding="utf-8",
        )
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)
    playback_service.invalidate(task_dir.name)
    return destination


class PlaybackService:
    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._sessions: dict[str, _PlaybackSession] = {}
        self._plan_cache: dict[Path, tuple[int, int, PlaybackPlan]] = {}
        self._prefix_cache: dict[str, tuple[int, int, float]] = {}

    def invalidate(self, task_id: str) -> None:
        with self._lock:
            self._prefix_cache.pop(task_id, None)

    def _load_plan(self, task_id: str) -> tuple[PlaybackPlan, int]:
        path = _safe_task_dir(task_id) / PLAN_FILENAME
        try:
            stat = path.stat()
        except FileNotFoundError as exc:
            raise PlaybackNotReadyError("播放清单尚未准备好") from exc
        with self._lock:
            cached = self._plan_cache.get(path)
            if cached and cached[0] == stat.st_mtime_ns and cached[1] == stat.st_size:
                return cached[2], stat.st_mtime_ns

        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            if data.get("version") != 1 or not isinstance(data.get("segments"), list):
                raise ValueError
            segments = []
            for position, raw in enumerate(data["segments"]):
                init_name = str(raw.get("init_name") or "")
                if init_name and (Path(init_name).name != init_name or not init_name.endswith(".init")):
                    raise ValueError
                index = int(raw["index"])
                if index != position:
                    raise ValueError
                segments.append(
                    PlaybackSegment(
                        index=index,
                        duration=max(0.001, float(raw.get("duration") or 0)),
                        discontinuity=bool(raw.get("discontinuity")),
                        init_name=init_name,
                    )
                )
            plan = PlaybackPlan(
                total_duration=max(0.0, float(data.get("total_duration") or 0)),
                target_duration=max(1, int(data.get("target_duration") or 1)),
                is_fmp4=bool(data.get("is_fmp4")),
                segments=tuple(segments),
            )
        except (OSError, TypeError, ValueError, json.JSONDecodeError) as exc:
            raise PlaybackError("本地播放清单损坏") from exc

        with self._lock:
            self._plan_cache[path] = (stat.st_mtime_ns, stat.st_size, plan)
        return plan, stat.st_mtime_ns

    def _available_prefix(
        self,
        task_id: str,
        plan: PlaybackPlan,
        plan_stamp: int,
    ) -> tuple[int, float]:
        seg_dir = _safe_task_dir(task_id) / "segments"
        with self._lock:
            cached_stamp, count, duration = self._prefix_cache.get(
                task_id,
                (plan_stamp, 0, 0.0),
            )
        if cached_stamp != plan_stamp or count > len(plan.segments):
            count, duration = 0, 0.0
        if count:
            last_path = seg_dir / f"{plan.segments[count - 1].index:06d}.seg"
            if not last_path.exists() or last_path.stat().st_size <= 0:
                count, duration = 0, 0.0

        while count < len(plan.segments):
            segment = plan.segments[count]
            path = seg_dir / f"{segment.index:06d}.seg"
            try:
                if path.stat().st_size <= 0:
                    break
            except FileNotFoundError:
                break
            duration += segment.duration
            count += 1

        with self._lock:
            self._prefix_cache[task_id] = (plan_stamp, count, duration)
        return count, duration

    def snapshot(self, task_id: str, status: str, output_path: str = "") -> PlaybackSnapshot:
        if status == "done" and output_path:
            output = Path(output_path)
            if output.exists() and output.is_file() and output.stat().st_size > 0:
                try:
                    plan, _ = self._load_plan(task_id)
                    total_duration = plan.total_duration
                    total_segments = len(plan.segments)
                except PlaybackError:
                    total_duration = 0.0
                    total_segments = 0
                return PlaybackSnapshot(
                    ready=True,
                    mode="file",
                    available_segments=total_segments,
                    total_segments=total_segments,
                    available_duration=total_duration,
                    total_duration=total_duration,
                    complete=True,
                )

        plan, stamp = self._load_plan(task_id)
        count, duration = self._available_prefix(task_id, plan, stamp)
        complete = bool(plan.segments) and count == len(plan.segments)
        ready = count > 0 and (duration >= MIN_START_DURATION or complete)
        return PlaybackSnapshot(
            ready=ready,
            mode="hls",
            available_segments=count,
            total_segments=len(plan.segments),
            available_duration=duration,
            total_duration=plan.total_duration,
            complete=complete,
        )

    def open_session(self, task_id: str) -> str:
        session_id = uuid.uuid4().hex
        now = time.monotonic()
        with self._lock:
            self._sessions[session_id] = _PlaybackSession(task_id=task_id, last_seen=now)
        return session_id

    def open_ready_session(
        self,
        task_id: str,
        status: str,
        output_path: str = "",
    ) -> tuple[str, PlaybackSnapshot]:
        with self._lock:
            snapshot = self.snapshot(task_id, status, output_path)
            if not snapshot.ready:
                raise PlaybackNotReadyError("至少需要 6 秒连续分片才能开始播放")
            session_id = uuid.uuid4().hex
            self._sessions[session_id] = _PlaybackSession(
                task_id=task_id,
                last_seen=time.monotonic(),
            )
            return session_id, snapshot

    def request_seek(self, task_id: str, session_id: str, target_time: float) -> dict:
        """Record a seek target and return its exact HLS segment location."""
        self.touch(task_id, session_id)
        if not math.isfinite(target_time):
            raise PlaybackError("播放位置无效")
        plan, _ = self._load_plan(task_id)
        if not plan.segments:
            raise PlaybackNotReadyError("播放清单尚未准备好")

        bounded = max(0.0, min(float(target_time), max(0.0, plan.total_duration - 0.001)))
        elapsed = 0.0
        target_index = len(plan.segments) - 1
        segment_start = 0.0
        for index, segment in enumerate(plan.segments):
            if bounded < elapsed + segment.duration or index == len(plan.segments) - 1:
                target_index = index
                segment_start = elapsed
                break
            elapsed += segment.duration

        with self._lock:
            session = self._sessions.get(session_id)
            if session is None or session.task_id != task_id:
                raise PlaybackSessionError("播放会话已失效，请重新打开播放器")
            session.requested_index = target_index
            session.requested_time = bounded
        return {
            "time": bounded,
            "index": target_index,
            "segment_start": segment_start,
            "segment_end": segment_start + plan.segments[target_index].duration,
            "total_duration": plan.total_duration,
        }

    def touch(self, task_id: str, session_id: str) -> None:
        now = time.monotonic()
        with self._lock:
            session = self._sessions.get(session_id)
            if session is None or session.task_id != task_id:
                raise PlaybackSessionError("播放会话已失效，请重新打开播放器")
            if now - session.last_seen > SESSION_TTL_SECONDS:
                self._sessions.pop(session_id, None)
                raise PlaybackSessionError("播放会话已超时，请重新打开播放器")
            session.last_seen = now

    def close(self, task_id: str, session_id: str) -> bool:
        with self._lock:
            session = self._sessions.get(session_id)
            if session is None or session.task_id != task_id:
                return False
            self._sessions.pop(session_id, None)
            return True

    def close_task(self, task_id: str) -> None:
        with self._lock:
            for session_id in [
                key for key, session in self._sessions.items() if session.task_id == task_id
            ]:
                self._sessions.pop(session_id, None)
            self._prefix_cache.pop(task_id, None)

    def _expire_locked(self, now: float) -> set[str]:
        expired_tasks: set[str] = set()
        for session_id, session in list(self._sessions.items()):
            if now - session.last_seen > SESSION_TTL_SECONDS:
                expired_tasks.add(session.task_id)
                self._sessions.pop(session_id, None)
        return expired_tasks

    def expire(self) -> set[str]:
        now = time.monotonic()
        with self._lock:
            return self._expire_locked(now)

    def has_active(self, task_id: str) -> bool:
        with self._lock:
            self._expire_locked(time.monotonic())
            return any(session.task_id == task_id for session in self._sessions.values())

    def cleanup_if_inactive(self, task_id: str, cleanup) -> bool:
        with self._lock:
            self._expire_locked(time.monotonic())
            if any(session.task_id == task_id for session in self._sessions.values()):
                return False
            cleanup()
            return True

    def cleanup_if_no_active(self, task_ids: set[str], cleanup) -> bool:
        with self._lock:
            self._expire_locked(time.monotonic())
            if any(session.task_id in task_ids for session in self._sessions.values()):
                return False
            cleanup()
            return True

    def playlist(
        self,
        task_id: str,
        status: str,
        session_id: str,
        *,
        access_token: str = "",
        full: bool = False,
    ) -> str:
        self.touch(task_id, session_id)
        plan, stamp = self._load_plan(task_id)
        count, _ = self._available_prefix(task_id, plan, stamp)
        if count <= 0:
            raise PlaybackNotReadyError("首个连续分片尚未下载完成")

        session_query = quote(session_id, safe="")
        token_query = f"&token={quote(access_token, safe='')}" if access_token else ""
        mode_query = "&full=1" if full else ""
        lines = [
            "#EXTM3U",
            f"#EXT-X-VERSION:{7 if plan.is_fmp4 else 3}",
            f"#EXT-X-TARGETDURATION:{plan.target_duration}",
            "#EXT-X-MEDIA-SEQUENCE:0",
            f"#EXT-X-PLAYLIST-TYPE:{'VOD' if full else 'EVENT'}",
        ]
        active_map = ""
        visible_segments = plan.segments if full else plan.segments[:count]
        for segment in visible_segments:
            if segment.discontinuity:
                lines.append("#EXT-X-DISCONTINUITY")
            if segment.init_name and segment.init_name != active_map:
                lines.append(
                    f'#EXT-X-MAP:URI="maps/{segment.init_name}?session={session_query}{token_query}{mode_query}"'
                )
            active_map = segment.init_name
            lines.append(f"#EXTINF:{segment.duration:.6f},")
            lines.append(
                f"segments/{segment.index:06d}.seg?session={session_query}{token_query}{mode_query}"
            )
        terminal = status in {"done", "failed", "canceled", "unsupported"}
        if full or terminal or count == len(plan.segments):
            lines.append("#EXT-X-ENDLIST")
        return "\n".join(lines) + "\n"

    def segment_path(
        self,
        task_id: str,
        index: int,
        session_id: str,
        *,
        sparse: bool = False,
    ) -> tuple[Path, bool]:
        self.touch(task_id, session_id)
        plan, stamp = self._load_plan(task_id)
        if index < 0 or index >= len(plan.segments):
            raise PlaybackNotReadyError("该分片尚未准备好")
        if not sparse:
            count, _ = self._available_prefix(task_id, plan, stamp)
            if index >= count:
                raise PlaybackNotReadyError("该分片尚未准备好")
        segment = plan.segments[index]
        path = _safe_task_dir(task_id) / "segments" / f"{index:06d}.seg"
        if not path.exists() or path.stat().st_size <= 0:
            raise PlaybackNotReadyError("该分片尚未准备好")
        return path, bool(segment.init_name)

    async def wait_for_segment(
        self,
        task_id: str,
        index: int,
        session_id: str,
        *,
        sparse: bool = False,
        timeout: float = 45.0,
    ) -> tuple[Path, bool]:
        deadline = time.monotonic() + max(0.1, timeout)
        while True:
            try:
                return self.segment_path(task_id, index, session_id, sparse=sparse)
            except PlaybackNotReadyError:
                if time.monotonic() >= deadline:
                    raise
                await asyncio.sleep(0.2)

    def map_path(self, task_id: str, map_name: str, session_id: str) -> Path:
        self.touch(task_id, session_id)
        if Path(map_name).name != map_name or not map_name.endswith(".init"):
            raise PlaybackError("无效的 init map")
        plan, _ = self._load_plan(task_id)
        if map_name not in {segment.init_name for segment in plan.segments if segment.init_name}:
            raise PlaybackError("init map 不属于该任务")
        path = _safe_task_dir(task_id) / "maps" / map_name
        if not path.exists() or path.stat().st_size <= 0:
            raise PlaybackNotReadyError("init map 尚未准备好")
        return path


playback_service = PlaybackService()
