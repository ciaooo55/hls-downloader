"""Global download throttle shared by HTTP/HLS/DASH workers.

Limit is configured as KiB/s (0 = unlimited). Workers call await consume(n)
after each successful read so concurrent tasks share one budget.
"""

from __future__ import annotations

import asyncio
import time


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
        self._limit_bps = limit_bps
        if limit_bps <= 0:
            self._tokens = 0.0
        else:
            self._tokens = min(self._tokens, limit_bps)

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
        amount = max(0, int(nbytes or 0))
        if amount <= 0:
            return
        while True:
            async with self._lock:
                if self._limit_bps <= 0:
                    return
                now = time.monotonic()
                self._refill(now)
                if self._tokens >= amount:
                    self._tokens -= amount
                    return
                deficit = amount - self._tokens
                wait = deficit / self._limit_bps if self._limit_bps > 0 else 0.0
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
        self._buckets.pop(task_id, None)


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
    download_throttle.configure(getattr(settings, "download_speed_limit_kib", 0) or 0)
    await download_throttle.consume(nbytes)
