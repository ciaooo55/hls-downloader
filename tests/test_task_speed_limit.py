import asyncio
import time

import pytest

from backend.app.downloader.task_manager import TaskManager
from backend.app.downloader.throttle import (
    GlobalDownloadThrottle,
    task_throttles,
    throttle_bytes,
)
from backend.app.models import Task, TaskStatus


async def _async_noop(*args, **kwargs):
    return None


def test_per_task_bucket_paces_only_that_task(monkeypatch):
    from backend.app.config import settings

    monkeypatch.setattr(settings, "download_speed_limit_kib", 0, raising=False)
    limited = Task(id="limited", url="https://a.test/x", speed_limit_kib=64)
    free = Task(id="free", url="https://b.test/y")

    async def run():
        # The free task must not be paced by the limited task's bucket.
        started = time.monotonic()
        await throttle_bytes(1024 * 1024, free)
        assert time.monotonic() - started < 0.2

        # The limited task consumes its burst (one second of budget), then
        # further bytes must wait.
        await throttle_bytes(64 * 1024, limited)
        started = time.monotonic()
        await throttle_bytes(32 * 1024, limited)
        assert time.monotonic() - started >= 0.3

    try:
        asyncio.run(run())
    finally:
        task_throttles.drop("limited")
        task_throttles.drop("free")


def test_manager_set_speed_limit_clamps_persists_and_configures(monkeypatch):
    async def run():
        manager = TaskManager()
        task = Task(
            id="task-limit",
            url="https://example.test/vod.m3u8",
            status=TaskStatus.DOWNLOADING_SEGMENTS,
        )
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager, "_save_db", _async_noop)

        await manager.set_task_speed_limit(task.id, 512)
        assert task.speed_limit_kib == 512
        assert task_throttles.bucket(task.id).limit_bps == 512 * 1024

        # Out-of-range values clamp instead of failing.
        await manager.set_task_speed_limit(task.id, 99_999_999)
        assert task.speed_limit_kib == 1048576

        # Zero removes the cap and drops the bucket.
        await manager.set_task_speed_limit(task.id, 0)
        assert task.speed_limit_kib == 0
        assert task_throttles._buckets.get(task.id) is None

    try:
        asyncio.run(run())
    finally:
        task_throttles.drop("task-limit")


def test_task_event_and_bucket_registry_roundtrip():
    bucket = task_throttles.bucket("round")
    assert isinstance(bucket, GlobalDownloadThrottle)
    assert task_throttles.bucket("round") is bucket
    task_throttles.drop("round")
    assert task_throttles.bucket("round") is not bucket
    task_throttles.drop("round")

    manager = TaskManager()
    task = Task(id="evt", url="https://example.test/f.bin", speed_limit_kib=256)
    manager.tasks[task.id] = task
    event = manager._task_event(task)
    assert event["speed_limit_kib"] == 256
