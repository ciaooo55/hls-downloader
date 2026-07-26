import asyncio
import time

from backend.app.downloader.throttle import GlobalDownloadThrottle, TaskThrottleRegistry


def test_unlimited_throttle_is_noop():
    throttle = GlobalDownloadThrottle()
    throttle.configure(0)

    async def run():
        started = time.monotonic()
        await throttle.consume(1024 * 1024)
        return time.monotonic() - started

    elapsed = asyncio.run(run())
    assert elapsed < 0.2


def test_limited_throttle_slows_burst():
    throttle = GlobalDownloadThrottle()
    # 100 KiB/s, request 50 KiB twice => roughly >= 0.4s for second consume after first
    throttle.configure(100)

    async def run():
        await throttle.consume(50 * 1024)
        started = time.monotonic()
        await throttle.consume(50 * 1024)
        return time.monotonic() - started

    elapsed = asyncio.run(run())
    assert elapsed >= 0.35


def test_configure_accepts_invalid_as_unlimited():
    throttle = GlobalDownloadThrottle()
    throttle.configure("bad")  # type: ignore[arg-type]
    assert throttle.limit_bps == 0.0


def test_settings_speed_limit_field_defaults_unlimited():
    from backend.app.config import Settings
    assert Settings().download_speed_limit_kib == 0


def test_chunk_larger_than_one_second_budget_completes():
    """A 256 KiB read under a 100 KiB/s cap must drain, not deadlock."""
    async def run():
        bucket = GlobalDownloadThrottle()
        bucket.configure(100)  # KiB/s
        started = time.monotonic()
        await asyncio.wait_for(bucket.consume(256 * 1024), timeout=10)
        assert time.monotonic() - started >= 1.0

    asyncio.run(run())


def test_dropping_a_task_bucket_releases_a_blocked_consumer():
    async def run():
        registry = TaskThrottleRegistry()
        bucket = registry.bucket("task-1")
        bucket.configure(1)  # 1 KiB/s: a 1 MiB read would take ~17 minutes
        waiter = asyncio.create_task(bucket.consume(1024 * 1024))
        await asyncio.sleep(0.05)
        assert not waiter.done()
        registry.drop("task-1")
        await asyncio.wait_for(waiter, timeout=3)

    asyncio.run(run())
