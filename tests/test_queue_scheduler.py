import asyncio
from datetime import datetime, timedelta, timezone

import pytest
from pydantic import ValidationError

from backend.app.models import Task, TaskStatus, TaskType
from backend.app.downloader.task_manager import TaskManager
from backend.app.schemas import TaskCreate


def test_queue_auto_start_time_is_fail_closed_and_time_aware(monkeypatch):
    manager = TaskManager()
    settings = __import__("backend.app.downloader.task_manager", fromlist=["settings"]).settings
    monkeypatch.setattr(settings, "queue_auto_start_enabled", True)
    monkeypatch.setattr(settings, "queue_auto_start_time", "21:30")
    assert not manager._queue_auto_start_due(datetime(2026, 7, 23, 21, 29))
    assert manager._queue_auto_start_due(datetime(2026, 7, 23, 21, 30))
    monkeypatch.setattr(settings, "queue_auto_start_time", "bad")
    assert not manager._queue_auto_start_due(datetime(2026, 7, 23, 22, 0))


def test_immediate_download_is_not_global_queue_managed(monkeypatch):
    manager = TaskManager()
    settings = __import__("backend.app.downloader.task_manager", fromlist=["settings"]).settings
    monkeypatch.setattr(settings, "queue_auto_start_enabled", False)
    monkeypatch.setattr(settings, "queue_auto_stop_enabled", False)
    assert manager._queue_managed_for_auto_start(True) is False
    monkeypatch.setattr(settings, "queue_auto_stop_enabled", True)
    assert manager._queue_managed_for_auto_start(True) is True
    assert manager._queue_managed_for_auto_start(False) is False


def test_weekday_and_overnight_queue_window(monkeypatch):
    manager = TaskManager()
    settings = __import__("backend.app.downloader.task_manager", fromlist=["settings"]).settings
    monkeypatch.setattr(settings, "queue_auto_start_enabled", True)
    monkeypatch.setattr(settings, "queue_auto_start_time", "22:00")
    monkeypatch.setattr(settings, "queue_auto_stop_enabled", True)
    monkeypatch.setattr(settings, "queue_auto_stop_time", "06:00")
    monkeypatch.setattr(settings, "queue_active_days", [0])  # Monday start

    assert manager._queue_schedule_state(datetime(2026, 8, 3, 23, 0)) == (True, False)
    assert manager._queue_schedule_state(datetime(2026, 8, 4, 5, 59)) == (True, False)
    assert manager._queue_schedule_state(datetime(2026, 8, 4, 6, 1)) == (False, True)
    assert manager._queue_schedule_state(datetime(2026, 8, 5, 23, 0)) == (False, False)

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


def test_per_task_schedule_due_and_window_validation():
    manager = TaskManager()
    now = datetime(2026, 8, 1, 12, 0)
    task = Task(id="scheduled", url="https://example.test/file")
    task.engine_state["scheduled_start_at"] = (now + timedelta(minutes=1)).isoformat()
    assert not manager._task_scheduled_start_due(task, now)
    task.engine_state["scheduled_start_at"] = (now - timedelta(seconds=1)).isoformat()
    assert manager._task_scheduled_start_due(task, now)

    with pytest.raises(ValidationError, match="停止时间"):
        TaskCreate(
            url="https://example.test/file",
            scheduled_start_at=now,
            scheduled_stop_at=now - timedelta(minutes=1),
        )


def test_task_schedule_is_normalized_to_utc():
    value = TaskCreate(
        url="https://example.test/file",
        scheduled_start_at=datetime(2026, 8, 1, 12, 0, tzinfo=timezone(timedelta(hours=8))),
    )
    assert value.scheduled_start_at == datetime(2026, 8, 1, 4, 0, tzinfo=timezone.utc)


def test_schedule_maintenance_starts_and_stops_due_tasks(monkeypatch):
    manager = TaskManager()
    settings = __import__("backend.app.downloader.task_manager", fromlist=["settings"]).settings
    monkeypatch.setattr(settings, "queue_auto_start_enabled", False)
    now = datetime(2026, 8, 1, 12, 0)
    start = Task(id="start", url="https://example.test/start", status=TaskStatus.QUEUED)
    start.engine_state.update({
        "queue_waiting_for_schedule": True,
        "scheduled_start_at": (now - timedelta(seconds=1)).isoformat(),
    })
    stop = Task(id="stop", url="https://example.test/stop", status=TaskStatus.DOWNLOADING)
    stop.engine_state["scheduled_stop_at"] = (now - timedelta(seconds=1)).isoformat()
    manager.tasks = {start.id: start, stop.id: stop}
    actions: list[tuple[str, str]] = []

    async def save(_task):
        return None

    async def start_task(task_id):
        actions.append(("start", task_id))

    async def pause_task(task_id):
        actions.append(("pause", task_id))

    monkeypatch.setattr(manager, "_save_db", save)
    monkeypatch.setattr(manager, "start_task", start_task)
    monkeypatch.setattr(manager, "pause_task", pause_task)

    asyncio.run(manager._maintain_scheduled_tasks(now))

    assert actions == [("start", "start"), ("pause", "stop")]
    assert "queue_waiting_for_schedule" not in start.engine_state
    assert stop.engine_state["scheduled_stop_handled"] is True

