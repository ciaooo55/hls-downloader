import asyncio

import pytest

from backend.app.downloader import task_manager as manager_module
from backend.app.downloader.hls import HLSDownloader
from backend.app.downloader.task_manager import TaskConflictError, TaskManager
from backend.app.models import Task, TaskStatus


def _task(task_id: str = "task1", status: TaskStatus = TaskStatus.QUEUED) -> Task:
    return Task(id=task_id, url="https://example.test/vod.m3u8", status=status)


def test_repeated_start_is_rejected_without_starting_duplicate(monkeypatch):
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
        with pytest.raises(TaskConflictError, match="已经在运行"):
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
        failed = manager.tasks["failed1"]
        assert failed.error_code == "HTTP_403"
        assert failed.error_stage == "downloading_segments"
        assert failed.http_status == 403
        assert failed.error_hint == "检查请求头"

    asyncio.run(run())


def test_retry_clears_structured_failure_fields(monkeypatch):
    async def run():
        manager = TaskManager()
        task = _task(status=TaskStatus.FAILED)
        task.error_message = "failed"
        task.error_code = "HTTP_403"
        task.error_stage = "parsing"
        task.error_url = "https://example.test/vod.m3u8"
        task.error_hint = "检查请求头"
        task.http_status = 403
        task.error_attempt = 5
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager, "_save_db", _async_noop)
        monkeypatch.setattr(manager, "start_task", _async_noop)

        await manager.retry_task(task.id)

        assert task.error_message == ""
        assert task.error_code == ""
        assert task.error_stage == ""
        assert task.error_url == ""
        assert task.error_hint == ""
        assert task.http_status == 0
        assert task.error_attempt == 0

    asyncio.run(run())


def test_task_event_contains_structured_failure_details():
    manager = TaskManager()
    task = _task(status=TaskStatus.FAILED)
    task.error_code = "HTTP_429"
    task.error_stage = "downloading_segments"
    task.error_url = "https://cdn.example.test/1.ts"
    task.error_hint = "降低并发"
    task.http_status = 429
    task.error_attempt = 5

    event = manager._task_event(task)

    assert event["error_code"] == "HTTP_429"
    assert event["error_stage"] == "downloading_segments"
    assert event["error_url"] == "https://cdn.example.test/1.ts"
    assert event["error_hint"] == "降低并发"
    assert event["http_status"] == 429
    assert event["error_attempt"] == 5


def test_available_actions_follow_backend_state_and_live_handle():
    class LiveHandle:
        @staticmethod
        def done():
            return False

    manager = TaskManager()
    queued = _task("queued", TaskStatus.QUEUED)
    manager.tasks[queued.id] = queued
    assert "start" in manager.get_available_actions(queued)

    queued.task_handle = LiveHandle()
    assert "start" not in manager.get_available_actions(queued)
    assert "cancel" in manager.get_available_actions(queued)

    parsing = _task("parsing", TaskStatus.PARSING)
    parsing.pause_event = asyncio.Event()
    assert "pause" not in manager.get_available_actions(parsing)

    downloading = _task("segments", TaskStatus.DOWNLOADING_SEGMENTS)
    downloading.pause_event = asyncio.Event()
    assert "pause" in manager.get_available_actions(downloading)

    downloading.progress.total_segments = 10
    downloading.progress.playable_segments = 2
    downloading.progress.playable_duration = 8
    assert "preview" in manager.get_available_actions(downloading)

    downloading.progress.playable_duration = 2
    assert "preview" not in manager.get_available_actions(downloading)
    assert "delete" in manager.get_available_actions(downloading)
    assert "delete_files" in manager.get_available_actions(downloading)


def test_task_event_contains_available_actions_and_queue_position():
    class LiveHandle:
        @staticmethod
        def done():
            return False

    manager = TaskManager()
    first = _task("first", TaskStatus.QUEUED)
    second = _task("second", TaskStatus.QUEUED)
    first.created_at = "2026-01-01T00:00:00"
    second.created_at = "2026-01-01T00:00:01"
    first.task_handle = LiveHandle()
    second.task_handle = LiveHandle()
    manager.tasks = {first.id: first, second.id: second}

    event = manager._task_event(second)

    assert event["available_actions"] == ["cancel", "log", "delete", "delete_files"]
    assert event["queue_position"] == 2


def test_structured_failure_details_survive_database_reload(tmp_path, monkeypatch):
    from backend.app import database as database_module

    async def run():
        monkeypatch.setattr(database_module, "DB_PATH", tmp_path / "tasks.db")
        manager = TaskManager()
        task = await manager.create_task("https://example.test/vod.m3u8")
        task.status = TaskStatus.FAILED
        task.error_message = "[HTTP_403] HTTP 403 Forbidden"
        task.error_code = "HTTP_403"
        task.error_stage = "downloading_segments"
        task.error_url = "https://cdn.example.test/1.ts"
        task.error_hint = "检查请求头"
        task.http_status = 403
        task.error_attempt = 5
        task.progress.playable_segments = 7
        task.progress.playable_duration = 42.5
        task.progress.media_duration = 120.0
        await manager._save_db(task)

        restored = TaskManager()
        await restored.load_from_db()
        loaded = restored.tasks[task.id]

        assert loaded.error_code == "HTTP_403"
        assert loaded.error_stage == "downloading_segments"
        assert loaded.error_url == "https://cdn.example.test/1.ts"
        assert loaded.error_hint == "检查请求头"
        assert loaded.http_status == 403
        assert loaded.error_attempt == 5
        assert loaded.progress.playable_segments == 7
        assert loaded.progress.playable_duration == 42.5
        assert loaded.progress.media_duration == 120.0

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


def test_temp_root_is_removed_only_after_all_tasks_finish_successfully(tmp_path, monkeypatch):
    async def run():
        manager = TaskManager()
        first = _task("first", TaskStatus.DONE)
        second = _task("second", TaskStatus.DOWNLOADING_SEGMENTS)
        manager.tasks = {first.id: first, second.id: second}
        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "keep_temp_files", False)
        temp_root = tmp_path / ".tasks"
        (temp_root / "leftover").mkdir(parents=True)
        (temp_root / "leftover" / "partial.seg").write_bytes(b"partial")

        await manager._cleanup_temp_root_if_all_done()
        assert temp_root.exists()

        second.status = TaskStatus.DONE
        await manager._cleanup_temp_root_if_all_done()
        assert not temp_root.exists()

    asyncio.run(run())


def test_temp_root_is_preserved_for_failed_or_paused_tasks(tmp_path, monkeypatch):
    async def run():
        manager = TaskManager()
        manager.tasks = {
            "failed": _task("failed", TaskStatus.FAILED),
            "paused": _task("paused", TaskStatus.PAUSED),
        }
        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "keep_temp_files", False)
        temp_root = tmp_path / ".tasks"
        temp_root.mkdir()

        await manager._cleanup_temp_root_if_all_done()
        assert temp_root.exists()

    asyncio.run(run())


def test_deleting_last_task_removes_temp_root(tmp_path, monkeypatch):
    async def fake_run_db(*args, **kwargs):
        return None

    async def run():
        manager = TaskManager()
        task = _task("failed", TaskStatus.FAILED)
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "keep_temp_files", False)
        monkeypatch.setattr(manager_module, "run_db", fake_run_db)
        temp_root = tmp_path / ".tasks"
        (temp_root / task.id).mkdir(parents=True)

        await manager.delete_task(task.id)
        assert not temp_root.exists()

    asyncio.run(run())


def test_delete_task_and_files_removes_completed_output(tmp_path, monkeypatch):
    async def fake_run_db(*args, **kwargs):
        return None

    async def run():
        manager = TaskManager()
        output = tmp_path / "archive.zip"
        output.write_bytes(b"payload")
        task = _task("complete", TaskStatus.DONE)
        task.output_path = str(output)
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module, "run_db", fake_run_db)

        await manager.delete_task(task.id, delete_files=True)

        assert not output.exists()
        assert task.id not in manager.tasks

    asyncio.run(run())


def test_delete_incomplete_task_always_removes_reserved_output(tmp_path, monkeypatch):
    async def fake_run_db(*args, **kwargs):
        return None

    async def run():
        manager = TaskManager()
        reserved = tmp_path / "partial.exe"
        reserved.write_bytes(b"")
        task = _task("partial", TaskStatus.PAUSED)
        task.engine_state["reserved_output_path"] = str(reserved)
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "keep_temp_files", True)
        monkeypatch.setattr(manager_module, "run_db", fake_run_db)

        await manager.delete_task(task.id)

        assert not reserved.exists()

    asyncio.run(run())


def test_new_task_registration_waits_for_final_temp_cleanup(tmp_path, monkeypatch):
    cleanup_started = asyncio.Event()
    release_cleanup = asyncio.Event()

    async def fake_run_db(*args, **kwargs):
        return None

    async def delayed_to_thread(func, *args):
        cleanup_started.set()
        await release_cleanup.wait()
        func(*args)

    async def run():
        manager = TaskManager()
        done = _task("done", TaskStatus.DONE)
        manager.tasks[done.id] = done
        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "keep_temp_files", False)
        monkeypatch.setattr(manager_module, "run_db", fake_run_db)
        monkeypatch.setattr(manager_module.asyncio, "to_thread", delayed_to_thread)
        temp_root = tmp_path / ".tasks"
        temp_root.mkdir()

        cleanup = asyncio.create_task(manager._cleanup_temp_root_if_all_done())
        await cleanup_started.wait()
        create = asyncio.create_task(manager.create_task("https://example.test/new.m3u8"))
        await asyncio.sleep(0)
        assert not create.done()

        release_cleanup.set()
        await cleanup
        new_task = await create
        assert new_task.id in manager.tasks
        assert not temp_root.exists()

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
        "error_code": "HTTP_403" if status == "failed" else "",
        "error_stage": "downloading_segments" if status == "failed" else "",
        "error_url": "https://cdn.example.test/1.ts" if status == "failed" else "",
        "error_hint": "检查请求头" if status == "failed" else "",
        "http_status": 403 if status == "failed" else 0,
        "error_attempt": 5 if status == "failed" else 0,
        "output_path": "",
        "created_at": "2026-01-01T00:00:00",
        "updated_at": "2026-01-01T00:00:00",
        "started_at": "",
        "finished_at": "",
    }
