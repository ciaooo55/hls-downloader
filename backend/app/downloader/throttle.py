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


async def throttle_bytes(nbytes: int) -> None:
    """Convenience wrapper used by download engines."""
    from ..config import settings

    download_throttle.configure(getattr(settings, "download_speed_limit_kib", 0) or 0)
    await download_throttle.consume(nbytes)
