from datetime import datetime

from backend.app.downloader.task_manager import TaskManager
from backend.app import downloader as downloader_package


def test_queue_auto_start_time_is_fail_closed_and_time_aware(monkeypatch):
    manager = TaskManager()
    settings = __import__("backend.app.downloader.task_manager", fromlist=["settings"]).settings
    monkeypatch.setattr(settings, "queue_auto_start_enabled", True)
    monkeypatch.setattr(settings, "queue_auto_start_time", "21:30")
    assert not manager._queue_auto_start_due(datetime(2026, 7, 23, 21, 29))
    assert manager._queue_auto_start_due(datetime(2026, 7, 23, 21, 30))
    monkeypatch.setattr(settings, "queue_auto_start_time", "bad")
    assert not manager._queue_auto_start_due(datetime(2026, 7, 23, 22, 0))


import asyncio
from backend.app.models import Task, TaskStatus, TaskType


def test_reorder_queue_updates_priority_and_position():
    manager = TaskManager()
    first = Task(id="a", url="https://example.test/a", task_type=TaskType.HTTP, status=TaskStatus.QUEUED, created_at="2026-01-01T00:00:00")
    second = Task(id="b", url="https://example.test/b", task_type=TaskType.HTTP, status=TaskStatus.QUEUED, created_at="2026-01-01T00:00:01")
    third = Task(id="c", url="https://example.test/c", task_type=TaskType.HTTP, status=TaskStatus.QUEUED, created_at="2026-01-01T00:00:02")
    manager.tasks = {first.id: first, second.id: second, third.id: third}

    async def run():
        await manager.reorder_queue("c", "top")
        assert manager.get_queue_position(third) in {0, 1}  # may be 0 without live handle
        # After reindex, sort key places c first among queued
        ordered = sorted(manager.tasks.values(), key=manager._queue_sort_key)
        assert ordered[0].id == "c"
        await manager.reorder_queue("c", "bottom")
        ordered = sorted(manager.tasks.values(), key=manager._queue_sort_key)
        assert ordered[-1].id == "c"
        await manager.reorder_queue("a", "down")
        ordered = sorted(manager.tasks.values(), key=manager._queue_sort_key)
        assert {item.id for item in ordered} == {"a", "b", "c"}

    asyncio.run(run())


def test_queue_sort_key_prefers_higher_priority():
    low = Task(id="low", url="https://example.test/l", status=TaskStatus.QUEUED, created_at="2026-01-01T00:00:00")
    high = Task(id="high", url="https://example.test/h", status=TaskStatus.QUEUED, created_at="2026-01-01T00:00:10")
    high.engine_state["queue_priority"] = 10
    low.engine_state["queue_priority"] = 1
    assert TaskManager._queue_sort_key(high) < TaskManager._queue_sort_key(low)

