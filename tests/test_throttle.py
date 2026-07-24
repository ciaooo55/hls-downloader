import asyncio
import time

from backend.app.downloader.throttle import GlobalDownloadThrottle


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
