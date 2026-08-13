import asyncio

from backend.app.config import Settings
from backend.app.downloader.task_manager import (
    TaskManager,
    auto_retry_delay_seconds,
    should_auto_retry_failed_task,
)
from backend.app.models import Task, TaskStatus


def test_auto_retry_defaults_off():
    loaded = Settings()
    assert loaded.auto_retry_failed_max == 0
    assert loaded.config_version == 27


def test_should_auto_retry_skips_permanent_and_respects_limit():
    failed = Task(id="a", url="https://example.test/a", status=TaskStatus.FAILED, error_code="HTTP_503", http_status=503)
    assert should_auto_retry_failed_task(failed, limit=0) is False
    assert should_auto_retry_failed_task(failed, limit=3) is True
    failed.engine_state["auto_retry_count"] = 3
    assert should_auto_retry_failed_task(failed, limit=3) is False

    denied = Task(id="b", url="https://example.test/b", status=TaskStatus.FAILED, error_code="HTTP_403", http_status=403)
    assert should_auto_retry_failed_task(denied, limit=3) is False
    checksum = Task(id="c", url="https://example.test/c", status=TaskStatus.FAILED, error_code="CHECKSUM_MISMATCH")
    assert should_auto_retry_failed_task(checksum, limit=3) is False
    paused = Task(id="d", url="https://example.test/d", status=TaskStatus.PAUSED)
    assert should_auto_retry_failed_task(paused, limit=3) is False
    assert auto_retry_delay_seconds(0) == 5
    assert auto_retry_delay_seconds(3) == 40
    assert auto_retry_delay_seconds(9) == 60


def test_failed_runner_schedules_auto_retry_then_manual_retry_cancels(monkeypatch):
    settings = __import__("backend.app.downloader.task_manager", fromlist=["settings"]).settings
    monkeypatch.setattr(settings, "auto_retry_failed_max", 2)

    async def immediate(_delay):
        return None

    monkeypatch.setattr(asyncio, "sleep", immediate)
    manager = TaskManager()
    task = Task(id="fail1", url="https://example.test/file.bin", status=TaskStatus.FAILED, error_code="HTTP_503", http_status=503)
    manager.tasks[task.id] = task
    retried = []

    async def fake_retry(task_id):
        retried.append(task_id)

    async def fake_save(_task):
        return None

    monkeypatch.setattr(manager, "retry_task", fake_retry)
    monkeypatch.setattr(manager, "_save_db", fake_save)

    async def run():
        manager._schedule_auto_retry(task)
        assert task.id in manager._auto_retry_handles
        manager._cancel_auto_retry(task.id)
        await asyncio.sleep(0)
        assert retried == []
        manager._schedule_auto_retry(task)
        await manager._auto_retry_handles[task.id]
        assert retried == ["fail1"]
        assert task.engine_state["auto_retry_count"] == 1

    asyncio.run(run())
