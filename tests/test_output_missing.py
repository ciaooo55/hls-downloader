import asyncio
from pathlib import Path

import pytest

from backend.app.downloader.task_manager import (
    TaskConflictError,
    TaskManager,
    task_output_missing,
)
from backend.app.models import Task, TaskStatus, TaskType


async def _async_noop(*_args, **_kwargs):
    return None


def _done_task(tmp_path: Path, *, create_file: bool, name: str = "video.mp4") -> Task:
    output = tmp_path / name
    if create_file:
        output.write_bytes(b"ok")
    return Task(
        id="done-1",
        url="https://cdn.example.test/video.mp4",
        task_type=TaskType.HTTP,
        status=TaskStatus.DONE,
        output_path=str(output),
        filename=name,
        engine_state={"output_is_file": True},
    )


def test_task_output_missing_only_applies_to_completed_tasks(tmp_path):
    active = Task(
        id="active",
        url="https://cdn.example.test/video.mp4",
        status=TaskStatus.DOWNLOADING,
        output_path=str(tmp_path / "partial.bin"),
    )
    assert task_output_missing(active) is False
    present = _done_task(tmp_path, create_file=True)
    assert task_output_missing(present) is False
    missing = _done_task(tmp_path, create_file=False, name="gone.mp4")
    assert task_output_missing(missing) is True
    empty = Task(id="empty", url="https://cdn.example.test/a.bin", status=TaskStatus.DONE, output_path="")
    assert task_output_missing(empty) is True


def test_missing_completed_file_offers_retry_not_launch(tmp_path):
    manager = TaskManager()
    task = _done_task(tmp_path, create_file=False)
    manager.tasks[task.id] = task
    actions = manager.get_available_actions(task)
    assert "retry" in actions
    assert "launch" not in actions
    assert "preview" not in actions
    assert "open" in actions
    event = manager._task_event(task)
    assert event["output_missing"] is True
    assert event["output_is_file"] is True


def test_present_completed_file_keeps_launch_and_rejects_retry(tmp_path):
    manager = TaskManager()
    task = _done_task(tmp_path, create_file=True)
    manager.tasks[task.id] = task
    actions = manager.get_available_actions(task)
    assert "launch" in actions
    assert "open" in actions
    assert "retry" not in actions
    assert manager._task_event(task)["output_missing"] is False

    async def run():
        with pytest.raises(TaskConflictError, match="最终文件仍在"):
            await manager.retry_task(task.id)

    asyncio.run(run())


def test_retry_missing_completed_task_starts_redownload(tmp_path, monkeypatch):
    manager = TaskManager()
    task = _done_task(tmp_path, create_file=False)
    manager.tasks[task.id] = task
    started = []

    async def start_task(task_id):
        started.append(task_id)

    monkeypatch.setattr(manager, "_save_db", _async_noop)
    monkeypatch.setattr(manager, "start_task", start_task)

    async def run():
        await manager.retry_task(task.id)

    asyncio.run(run())
    assert started == [task.id]
    assert task.status is TaskStatus.QUEUED
    assert task.output_path == ""
    assert "丢失" in task.last_log
    assert task_output_missing(task) is False
