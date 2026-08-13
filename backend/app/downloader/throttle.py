"""Global download throttle shared by HTTP/HLS/DASH workers.

Limit is configured as KiB/s (0 = unlimited). Workers call await consume(n)
after each successful read so concurrent tasks share one budget.
"""

from __future__ import annotations

import asyncio
import time
from datetime import datetime


class GlobalDownloadThrottle:
    def __init__(self) -> None:
        self._limit_bps = 0.0
        self._tokens = 0.0
        self._updated = time.monotonic()
        self._lock = asyncio.Lock()

    def configure(self, limit_kib_per_sec: int | float | None) -> None:
        try:
            kib = max(0.0, float(limit_kib_per_sec or 0))
        except (TypeError, ValueError):
            kib = 0.0
        limit_bps = kib * 1024.0
        previous_limit = self._limit_bps
        self._limit_bps = limit_bps
        if limit_bps <= 0:
            self._tokens = 0.0
            self._updated = time.monotonic()
        else:
            self._tokens = min(self._tokens, limit_bps)
            # A new cap starts with no accumulated burst.  Without resetting
            # the timestamp, test/setup time before the first read becomes a
            # hidden free allowance and the configured speed is exceeded.
            # Repeated configure() calls at the same cap intentionally keep
            # the bucket state, as throttle_bytes invokes it for every chunk.
            if previous_limit != limit_bps:
                self._updated = time.monotonic()

    @property
    def limit_bps(self) -> float:
        return self._limit_bps

    def _refill(self, now: float) -> None:
        if self._limit_bps <= 0:
            self._updated = now
            return
        elapsed = max(0.0, now - self._updated)
        self._updated = now
        # Cap burst to one second of budget so speed settles quickly.
        self._tokens = min(self._limit_bps, self._tokens + elapsed * self._limit_bps)

    async def consume(self, nbytes: int) -> None:
        remaining = max(0, int(nbytes or 0))
        if remaining <= 0:
            return
        # Unlimited transfers must not serialize every chunk of every worker
        # on the shared lock. configure() and consume() only run on the API
        # event loop, so this unlocked read cannot observe a torn value.
        if self._limit_bps <= 0:
            return
        while True:
            async with self._lock:
                if self._limit_bps <= 0:
                    return
                now = time.monotonic()
                self._refill(now)
                # Partial grants: a read larger than one second of budget
                # (e.g. a 256 KiB chunk under a 100 KiB/s limit) drains over
                # several refills instead of waiting for a burst that the
                # one-second cap can never produce.
                take = min(remaining, self._tokens)
                if take > 0:
                    self._tokens -= take
                    remaining -= take
                if remaining <= 0:
                    return
                wait = min(remaining, self._limit_bps) / self._limit_bps
            await asyncio.sleep(min(1.0, max(0.001, wait)))


download_throttle = GlobalDownloadThrottle()


class TaskThrottleRegistry:
    """Per-task token buckets layered on top of the global budget."""

    def __init__(self) -> None:
        self._buckets: dict[str, GlobalDownloadThrottle] = {}

    def bucket(self, task_id: str) -> GlobalDownloadThrottle:
        found = self._buckets.get(task_id)
        if found is None:
            found = GlobalDownloadThrottle()
            self._buckets[task_id] = found
        return found

    def drop(self, task_id: str) -> None:
        # Release anyone already waiting on the old bucket: without this a
        # coroutine parked inside consume() would keep the removed limit.
        bucket = self._buckets.pop(task_id, None)
        if bucket is not None:
            bucket.configure(0)


task_throttles = TaskThrottleRegistry()


async def throttle_bytes(nbytes: int, task=None) -> None:
    """Consume from the task's own budget (when set), then the global one.

    Both limits must admit the bytes, so a per-task cap can never exceed
    the global cap and vice versa.
    """
    from ..config import settings

    if task is not None:
        limit = int(getattr(task, "speed_limit_kib", 0) or 0)
        if limit > 0:
            bucket = task_throttles.bucket(task.id)
            bucket.configure(limit)
            await bucket.consume(nbytes)
    download_throttle.configure(effective_download_speed_limit_kib())
    await download_throttle.consume(nbytes)


def _parse_hhmm(value: object) -> tuple[int, int] | None:
    try:
        hour_text, minute_text = str(value or '').strip().split(':', 1)
        hour, minute = int(hour_text), int(minute_text)
    except (TypeError, ValueError):
        return None
    if 0 <= hour <= 23 and 0 <= minute <= 59:
        return hour, minute
    return None


def _inside_speed_window(now: datetime, start: tuple[int, int], end: tuple[int, int]) -> bool:
    current = (now.hour, now.minute)
    if start < end:
        return start <= current < end
    return current >= start or current < end


def effective_download_speed_limit_kib(now: datetime | None = None) -> int:
    """Return the active global cap. Schedule is opt-in and fail-closed."""
    from ..config import settings

    base = max(0, min(1048576, int(getattr(settings, 'download_speed_limit_kib', 0) or 0)))
    if not getattr(settings, 'speed_schedule_enabled', False):
        return base
    start = _parse_hhmm(getattr(settings, 'speed_schedule_start', '08:00'))
    end = _parse_hhmm(getattr(settings, 'speed_schedule_end', '23:00'))
    if start is None or end is None or start == end:
        return base
    current = now or datetime.now()
    if _inside_speed_window(current, start, end):
        scheduled = max(0, min(1048576, int(getattr(settings, 'speed_schedule_limit_kib', 0) or 0)))
        return scheduled
    return base

