from __future__ import annotations

import asyncio
import os
import weakref
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable

from ..models import TaskStatus


_locks_by_loop: weakref.WeakKeyDictionary[
    asyncio.AbstractEventLoop, dict[str, asyncio.Lock]
] = weakref.WeakKeyDictionary()


def _existing_parent(path: Path) -> Path:
    candidate = path.expanduser().resolve(strict=False)
    while not candidate.exists() and candidate != candidate.parent:
        candidate = candidate.parent
    return candidate


def volume_key(path: Path) -> str:
    """Return a stable key for the filesystem volume containing *path*."""
    resolved = path.expanduser().resolve(strict=False)
    if os.name == "nt":
        anchor = resolved.anchor or resolved.drive
        return f"win:{anchor.casefold()}"
    existing = _existing_parent(resolved)
    try:
        return f"dev:{os.stat(existing).st_dev}"
    except OSError:
        return f"path:{resolved.anchor or '/'}"


def volume_keys(paths: Iterable[Path]) -> tuple[str, ...]:
    return tuple(sorted({volume_key(Path(path)) for path in paths}))


@dataclass
class PostprocessLease:
    _locks: list[asyncio.Lock]
    _released: bool = False

    def release(self) -> None:
        if self._released:
            return
        self._released = True
        for lock in reversed(self._locks):
            lock.release()


async def acquire_postprocess_lease(
    paths: Iterable[Path],
    *,
    task=None,
    waiting_stage: str = "merging",
    waiting_message: str = "正在等待同一磁盘上的其他任务完成后处理",
    on_progress: Callable | None = None,
    on_log: Callable | None = None,
) -> PostprocessLease:
    """Serialize heavy local-media work that touches any shared volume.

    Multiple FFmpeg remuxes reading thousands of segments from one HDD can
    reduce aggregate throughput to a few MiB/s.  Locks are acquired in stable
    key order so tasks that span a temp and output volume cannot deadlock.
    """
    loop = asyncio.get_running_loop()
    loop_locks = _locks_by_loop.setdefault(loop, {})
    locks = [loop_locks.setdefault(key, asyncio.Lock()) for key in volume_keys(paths)]
    acquired: list[asyncio.Lock] = []
    waiting = any(lock.locked() for lock in locks)
    if waiting and task is not None:
        task.status = (
            TaskStatus.REMUXING if waiting_stage == "remuxing" else TaskStatus.MERGING
        )
        task.stage = waiting_stage
        task.last_log = waiting_message
        if on_progress is not None:
            on_progress(task)
        if on_log is not None:
            on_log(task.id, f"[merge] {waiting_message}")
    try:
        for lock in locks:
            await lock.acquire()
            acquired.append(lock)
    except BaseException:
        for lock in reversed(acquired):
            lock.release()
        raise
    return PostprocessLease(acquired)
