import asyncio

import pytest

from backend.app.downloader import task_manager as manager_module
from backend.app.downloader.hls import HLSDownloader
from backend.app.downloader.task_manager import TaskConflictError, TaskManager
from backend.app.models import Task, TaskStatus


def _task(task_id: str = "task1", status: TaskStatus = TaskStatus.QUEUED) -> Task:
    return Task(id=task_id, url="https://example.test/vod.m3u8", status=status)


def test_start_task_is_idempotent(monkeypatch):
    started = 0
    release = asyncio.Event()

    class FakeDownloader:
        def __init__(self, task, on_progress, on_log):
            self.task = task

        async def run(self):
            nonlocal started
            started += 1
            self.task.status = TaskStatus.DOWNLOADING_SEGMENTS
            await release.wait()

    async def run():
        manager = TaskManager()
        task = _task()
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager_module, "HLSDownloader", FakeDownloader)
        monkeypatch.setattr(manager, "_save_db", _async_noop)

        await manager.start_task(task.id)
        first_handle = task.task_handle
        await manager.start_task(task.id)
        await asyncio.sleep(0)

        assert task.task_handle is first_handle
        assert started == 1
        release.set()
        await first_handle

    asyncio.run(run())


def test_pause_transitions_to_pausing_and_rejects_wrong_stage(monkeypatch):
    async def run():
        manager = TaskManager()
        active = _task(status=TaskStatus.DOWNLOADING_SEGMENTS)
        active.pause_event = asyncio.Event()
        manager.tasks[active.id] = active
        monkeypatch.setattr(manager, "_save_db", _async_noop)

        await manager.pause_task(active.id)
        assert active.status is TaskStatus.PAUSING
        assert active.pause_event.is_set()

        queued = _task("queued")
        manager.tasks[queued.id] = queued
        with pytest.raises(TaskConflictError):
            await manager.pause_task(queued.id)

    asyncio.run(run())


def test_cancel_waits_for_running_coroutine(monkeypatch):
    cleanup_finished = asyncio.Event()

    class FakeDownloader:
        def __init__(self, task, on_progress, on_log):
            self.task = task

        async def run(self):
            try:
                await asyncio.Event().wait()
            finally:
                await asyncio.sleep(0.01)
                cleanup_finished.set()

    async def run():
        manager = TaskManager()
        task = _task()
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager_module, "HLSDownloader", FakeDownloader)
        monkeypatch.setattr(manager, "_save_db", _async_noop)

        await manager.start_task(task.id)
        await asyncio.sleep(0)
        await manager.cancel_task(task.id)

        assert cleanup_finished.is_set()
        assert task.task_handle.done()
        assert task.status is TaskStatus.CANCELED

    asyncio.run(run())


def test_load_from_db_keeps_history_and_pauses_interrupted_tasks(monkeypatch):
    rows = [
        _db_row("done", task_id="done1"),
        _db_row("downloading_segments", task_id="interrupted"),
        _db_row("failed", task_id="failed1"),
    ]

    async def fake_run_db(sql, params=()):
        assert "status NOT IN" not in sql
        return rows

    async def run():
        manager = TaskManager()
        monkeypatch.setattr(manager_module, "run_db", fake_run_db)
        monkeypatch.setattr(manager, "_save_db", _async_noop)

        await manager.load_from_db()

        assert set(manager.tasks) == {"done1", "interrupted", "failed1"}
        assert manager.tasks["done1"].status is TaskStatus.DONE
        interrupted = manager.tasks["interrupted"]
        assert interrupted.status is TaskStatus.PAUSED
        assert interrupted.stage == "interrupted"
        assert interrupted.progress.completed_segments == 3
        assert interrupted.progress.post_percent == 25.0

    asyncio.run(run())


def test_downloader_shutdown_cancellation_preserves_partial_files(tmp_path, monkeypatch):
    from backend.app.downloader import hls as hls_module

    async def run():
        task = _task(status=TaskStatus.DOWNLOADING_M3U8)
        task.cancel_event = asyncio.Event()
        task.pause_event = asyncio.Event()
        monkeypatch.setattr(hls_module.settings, "download_dir", str(tmp_path))
        task_dir = tmp_path / ".tasks" / task.id
        task_dir.mkdir(parents=True)
        partial = task_dir / "partial.seg"
        partial.write_bytes(b"partial")

        downloader = HLSDownloader(task)
        handle = asyncio.create_task(downloader.run())
        await asyncio.sleep(0)
        handle.cancel()
        await handle

        assert task.status is TaskStatus.PAUSED
        assert task.stage == "interrupted"
        assert partial.exists()

    asyncio.run(run())


async def _async_noop(*args, **kwargs):
    return None


def _db_row(status: str, task_id: str) -> dict:
    return {
        "id": task_id,
        "title": task_id,
        "url": "https://example.test/vod.m3u8",
        "referer": "",
        "origin": "",
        "user_agent": "",
        "cookie": "",
        "filename": task_id,
        "concurrency": 4,
        "status": status,
        "stage": status,
        "last_log": status,
        "total_segments": 10,
        "completed_segments": 3,
        "failed_segments": 1,
        "downloaded_bytes": 100,
        "total_bytes": 200,
        "speed_bytes_per_sec": 10,
        "eta_seconds": 9,
        "post_percent": 25,
        "error_message": "",
        "output_path": "",
        "created_at": "2026-01-01T00:00:00",
        "updated_at": "2026-01-01T00:00:00",
        "started_at": "",
        "finished_at": "",
    }
