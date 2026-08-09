import asyncio
import os
from pathlib import Path

import pytest

from backend.app.downloader import task_manager as manager_module
from backend.app.downloader.hls import HLSDownloader
from backend.app.downloader.engine import task_work_dir
from backend.app.downloader.task_manager import TaskConflictError, TaskManager
from backend.app.models import Task, TaskStatus, TaskType


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


def test_resume_waits_for_visible_paused_task_to_finish_old_run_tail(monkeypatch):
    async def run():
        manager = TaskManager()
        task = _task(status=TaskStatus.PAUSED)
        release = asyncio.Event()
        started = False

        async def finish_old_run():
            await release.wait()

        old_handle = asyncio.create_task(finish_old_run())
        task.task_handle = old_handle
        manager.tasks[task.id] = task

        async def start_task(task_id: str):
            nonlocal started
            assert task_id == task.id
            assert old_handle.done()
            started = True

        monkeypatch.setattr(manager, "start_task", start_task)
        resume = asyncio.create_task(manager.resume_task(task.id))
        await asyncio.sleep(0)
        assert started is False
        release.set()
        await resume
        assert started is True

    asyncio.run(run())


def test_refresh_task_request_preserves_progress_and_credentials(monkeypatch):
    async def run():
        manager = TaskManager()
        task = Task(
            id="expired",
            url="https://cdn.test/video.mp4?token=old",
            task_type=TaskType.HTTP,
            status=TaskStatus.FAILED,
            cookie="old=1",
            request_headers={"referer": "https://site.test/old"},
        )
        task.progress.downloaded_bytes = 4096
        task.error_message = "HTTP 403"
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager, "_save_db", _async_noop)

        updated = await manager.refresh_task_request(
            task.id,
            url="https://cdn.test/video.mp4?token=new",
            cookie="new=2",
            request_headers={"Referer": "https://site.test/new"},
            auto_resume=False,
        )

        assert updated is task
        assert task.status is TaskStatus.PAUSED
        assert task.url.endswith("token=new")
        assert task.cookie == "new=2"
        assert task.request_headers == {"referer": "https://site.test/new"}
        assert task.progress.downloaded_bytes == 4096
        assert task.error_message == ""

    asyncio.run(run())


def test_refresh_waits_for_failed_runner_finalization(monkeypatch):
    async def run():
        manager = TaskManager()
        task = Task(
            id="failed-tail",
            url="https://cdn.test/video.mp4?s=old&e=1&_t=1",
            task_type=TaskType.HTTP,
            status=TaskStatus.FAILED,
        )
        manager.tasks[task.id] = task
        release = asyncio.Event()

        async def finish_runner():
            await release.wait()

        task.task_handle = asyncio.create_task(finish_runner())
        monkeypatch.setattr(manager, "_save_db", _async_noop)
        refreshing = asyncio.create_task(manager.refresh_task_request(
            task.id,
            url="https://cdn.test/video.mp4?s=fresh&e=2&_t=2",
            auto_resume=False,
        ))
        await asyncio.sleep(0)
        assert refreshing.done() is False

        release.set()
        updated = await refreshing

        assert updated.url.endswith("s=fresh&e=2&_t=2")
        assert updated.status is TaskStatus.PAUSED

    asyncio.run(run())


def test_new_signed_browser_url_revives_only_safe_stale_tasks():
    manager = TaskManager()
    failed = Task(
        id="failed",
        url="https://cdn.test/video.mp4?token=old",
        source_page_url="https://site.test/watch/1",
        task_type=TaskType.HTTP,
        status=TaskStatus.FAILED,
        updated_at="2026-01-02",
    )
    paused = Task(
        id="paused",
        url="https://cdn.test/video.mp4?token=older",
        task_type=TaskType.HTTP,
        status=TaskStatus.PAUSED,
        updated_at="2026-01-03",
    )
    interrupted = Task(
        id="interrupted",
        url="https://cdn.test/video.mp4?token=interrupted",
        task_type=TaskType.HTTP,
        status=TaskStatus.PAUSED,
        updated_at="2026-01-04",
        engine_state={"state_reason": "core_interrupted"},
    )
    probing = Task(
        id="probing",
        url="https://cdn.test/video.mp4?token=probe",
        task_type=TaskType.HTTP,
        status=TaskStatus.DOWNLOADING,
        stage="probing",
        updated_at="2026-01-05",
    )
    probing.task_handle = type("LiveHandle", (), {"done": staticmethod(lambda: False)})()
    manager.tasks = {
        failed.id: failed,
        paused.id: paused,
        interrupted.id: interrupted,
        probing.id: probing,
    }

    assert manager.find_expired_request_task(
        "https://cdn.test/video.mp4?token=fresh",
        "https://site.test/watch/1",
    ) is probing

    manager.tasks.pop(probing.id)
    assert manager.find_expired_request_task(
        "https://cdn.test/video.mp4?token=fresh",
        "https://site.test/other",
    ) is interrupted

    manager.tasks.pop(interrupted.id)
    assert manager.find_expired_request_task(
        "https://cdn.test/video.mp4?token=fresh",
        "https://site.test/other",
    ) is failed

    failed_live = Task(
        id="failed-live",
        url="https://cdn.test/video.mp4?token=live-old",
        source_page_url="https://site.test/watch/live",
        task_type=TaskType.HLS,
        status=TaskStatus.FAILED,
        updated_at="2026-01-06",
        engine_state={"live": True},
    )
    running_live = Task(
        id="running-live",
        url="https://cdn.test/video.mp4?token=live-running",
        source_page_url="https://site.test/watch/live",
        task_type=TaskType.HLS,
        status=TaskStatus.DOWNLOADING_SEGMENTS,
        updated_at="2026-01-07",
        engine_state={"live": True},
    )
    running_live.task_handle = type("LiveHandle", (), {"done": staticmethod(lambda: False)})()
    manager.tasks = {failed_live.id: failed_live, running_live.id: running_live}
    assert manager.find_expired_request_task(
        "https://cdn.test/video.mp4?token=live-fresh",
        "https://site.test/watch/live",
    ) is failed_live

    signed_short = Task(
        id="signed-short",
        url="https://old-edge.test/asset.mp4?quality=1080&s=old&e=1&_t=2",
        source_page_url="https://site.test/watch/signed",
        task_type=TaskType.HTTP,
        status=TaskStatus.FAILED,
    )
    manager.tasks = {signed_short.id: signed_short}
    assert manager.find_expired_request_task(
        "https://new-edge.test/asset.mp4?e=9&s=new&_t=8&quality=1080",
        "https://site.test/watch/signed",
    ) is signed_short
    assert manager.find_expired_request_task(
        "https://new-edge.test/asset.mp4?e=9&s=new&_t=8&quality=720",
        "https://site.test/watch/signed",
    ) is None


def test_finished_runner_cannot_leave_task_in_active_state(monkeypatch):
    async def crash():
        raise RuntimeError("engine crash")

    async def run():
        manager = TaskManager()
        task = _task("unexpected-exit", TaskStatus.DOWNLOADING)
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager, "_save_db", _async_noop)
        task.task_handle = asyncio.create_task(crash())
        task.task_handle.add_done_callback(lambda handle: manager._on_task_finished(task, handle))
        with pytest.raises(RuntimeError, match="engine crash"):
            await task.task_handle
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        assert task.status is TaskStatus.FAILED
        assert task.error_code == "DOWNLOADER_UNEXPECTED_EXIT"
        assert task.stage == "failed"
        assert "卡在准备下载" in task.last_log

    asyncio.run(run())


def test_site_profile_fills_manual_task_defaults(monkeypatch):
    async def run():
        manager = TaskManager()
        monkeypatch.setattr(manager_module.settings, "site_profiles", [{
            "host": "*.example.test",
            "referer": "https://example.test/watch",
            "concurrency": 3,
            "speed_limit_kib": 512,
            "request_headers": {"X-Site-Token": "profile"},
        }])
        monkeypatch.setattr(manager_module, "run_db", _async_noop)

        task = await manager.create_task("https://cdn.example.test/file.bin")

        assert task.referer == "https://example.test/watch"
        assert task.concurrency == 3
        assert task.speed_limit_kib == 512
        assert task.request_headers == {"x-site-token": "profile"}

    asyncio.run(run())


def test_update_restart_marks_only_running_tasks(monkeypatch):
    async def run():
        manager = TaskManager()
        active = _task("active", TaskStatus.DOWNLOADING_SEGMENTS)
        waiting_for_slot = _task("waiting", TaskStatus.QUEUED)
        waiting_for_slot.task_handle = type("LiveHandle", (), {"done": staticmethod(lambda: False)})()
        user_paused = _task("paused", TaskStatus.PAUSED)
        queued = _task("queued", TaskStatus.QUEUED)
        manager.tasks = {
            active.id: active,
            waiting_for_slot.id: waiting_for_slot,
            user_paused.id: user_paused,
            queued.id: queued,
        }
        monkeypatch.setattr(manager, "_save_db", _async_noop)

        assert await manager.prepare_for_update_restart() == 2
        assert active.engine_state[manager_module.RESUME_AFTER_UPDATE_KEY] is True
        assert waiting_for_slot.engine_state[manager_module.RESUME_AFTER_UPDATE_KEY] is True
        assert manager_module.RESUME_AFTER_UPDATE_KEY not in user_paused.engine_state
        assert manager_module.RESUME_AFTER_UPDATE_KEY not in queued.engine_state

    asyncio.run(run())


def test_torrent_file_selection_can_change_while_downloading(monkeypatch):
    class LiveTorrentDownloader:
        def __init__(self):
            self.selected = None

        def select_files(self, indexes):
            self.selected = indexes

    async def run():
        manager = TaskManager()
        task = Task(
            id="torrent-live-selection",
            url="magnet:?xt=urn:btih:test",
            task_type=TaskType.TORRENT,
            status=TaskStatus.DOWNLOADING,
            engine_state={
                "files": [
                    {"index": 0, "path": "episode-01.mkv", "size": 100},
                    {"index": 1, "path": "extras.txt", "size": 10},
                ],
                "selected_files": [0, 1],
            },
        )
        downloader = LiveTorrentDownloader()
        manager.tasks[task.id] = task
        manager._downloaders[task.id] = downloader
        monkeypatch.setattr(manager_module, "TorrentDownloader", LiveTorrentDownloader)
        monkeypatch.setattr(manager, "_save_db", _async_noop)

        await manager.select_torrent_files(task.id, [0])

        assert task.status is TaskStatus.DOWNLOADING
        assert task.engine_state["selected_files"] == [0]
        assert downloader.selected == [0]

    asyncio.run(run())


def test_torrent_waits_for_explicit_file_selection_before_it_can_start():
    async def scenario():
        manager = TaskManager()
        task = Task(
            id="torrent-awaiting-selection",
            url="torrent-file:bundle.torrent",
            task_type=TaskType.TORRENT,
            status=TaskStatus.AWAITING_SELECTION,
            engine_state={
                "files": [{"index": 0, "path": "movie.mkv", "size": 1}],
                "selected_files": [0],
            },
        )
        manager.tasks[task.id] = task
        assert "start" in manager.get_available_actions(task)

    asyncio.run(scenario())


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

    async def fake_iter_db_rows(sql, params=(), **_kwargs):
        assert "status NOT IN" not in sql
        for item in rows:
            yield item

    async def run():
        manager = TaskManager()
        monkeypatch.setattr(manager_module, "iter_db_rows", fake_iter_db_rows)
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


def test_load_from_db_auto_resumes_only_update_marked_tasks(monkeypatch):
    row = _db_row("paused", task_id="update-restart")
    row["engine_state"] = '{"resume_after_update": true}'

    async def fake_iter_db_rows(sql, params=(), **_kwargs):
        yield row

    async def run():
        manager = TaskManager()
        started = []

        async def fake_start(task_id):
            started.append(task_id)

        monkeypatch.setattr(manager_module, "iter_db_rows", fake_iter_db_rows)
        monkeypatch.setattr(manager, "_save_db", _async_noop)
        monkeypatch.setattr(manager, "start_task", fake_start)

        await manager.load_from_db()

        task = manager.tasks["update-restart"]
        assert started == [task.id]
        assert manager_module.RESUME_AFTER_UPDATE_KEY not in task.engine_state
        assert task.last_log == "更新完成，正在自动继续下载"

    asyncio.run(run())


def test_load_from_db_does_not_auto_resume_before_legal_acceptance(monkeypatch):
    row = _db_row("paused", task_id="legal-gated-update")
    row["engine_state"] = '{"resume_after_update": true}'

    async def fake_iter_db_rows(sql, params=(), **_kwargs):
        yield row

    async def run():
        manager = TaskManager()
        started = []

        async def fake_start(task_id):
            started.append(task_id)

        monkeypatch.setattr(manager_module, "iter_db_rows", fake_iter_db_rows)
        monkeypatch.setattr(manager, "_save_db", _async_noop)
        monkeypatch.setattr(manager, "start_task", fake_start)

        await manager.load_from_db(auto_start_allowed=False)

        task = manager.tasks["legal-gated-update"]
        assert started == []
        assert task.status is TaskStatus.PAUSED
        assert task.stage == "paused"
        assert task.engine_state["state_reason"] == "legal_terms_required"
        assert "同意用户协议" in task.last_log

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

    downloading.progress.playable_duration = 0.5
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

    assert set(event["available_actions"]) >= {"cancel", "log", "delete", "delete_files"}
    assert {"queue_up", "queue_top"}.issubset(event["available_actions"])
    assert "start" not in event["available_actions"]
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


def test_private_browser_request_headers_are_encrypted_and_survive_reload(tmp_path, monkeypatch):
    from backend.app import database as database_module
    import base64

    async def run():
        monkeypatch.setattr(database_module, "DB_PATH", tmp_path / "tasks.db")
        manager = TaskManager()
        task = await manager.create_task(
            "https://cdn.example.test/protected.bin",
            request_headers={
                "Authorization": "Bearer signed-token",
                "Sec-CH-UA": '"Chromium";v="140"',
                "X-Playback-Token": "opaque",
                "Host": "attacker.test",
                "Range": "bytes=0-1",
                "Cookie": "must-use-cookie-field=1",
                "Content-Type": "application/json",
            },
            request_method="POST",
            request_body=base64.b64encode(b'{"token":"post-secret"}').decode("ascii"),
            cookie="session=secret",
            selected_video="https://cdn.example.test/video.m3u8?token=selected-secret",
            selected_audio="https://cdn.example.test/audio.m3u8?token=audio-secret",
            request_contexts={
                "https://segments.example.test": {
                    "request_headers": {"Authorization": "Bearer segments"},
                    "cookie": "segment_session=private",
                }
            },
        )

        rows = await database_module.run_db(
            "SELECT url,source_page_url,referer,origin,request_headers,request_contexts,request_body,cookie,selected_video,selected_audio FROM tasks WHERE id=?", (task.id,)
        )
        stored = rows[0]
        assert "protected.bin" not in stored["url"]
        assert "signed-token" not in stored["request_headers"]
        assert "Bearer segments" not in stored["request_contexts"]
        assert "segment_session=private" not in stored["request_contexts"]
        assert "post-secret" not in stored["request_body"]
        assert "session=secret" not in stored["cookie"]
        assert "selected-secret" not in stored["selected_video"]
        assert "audio-secret" not in stored["selected_audio"]

        restored = TaskManager()
        await restored.load_from_db()
        loaded = restored.tasks[task.id]
        assert loaded.request_headers == {
            "authorization": "Bearer signed-token",
            "sec-ch-ua": '"Chromium";v="140"',
            "x-playback-token": "opaque",
            "content-type": "application/json",
        }
        assert loaded.request_method == "POST"
        assert loaded.url == "https://cdn.example.test/protected.bin"
        assert base64.b64decode(loaded.request_body) == b'{"token":"post-secret"}'
        assert loaded.cookie == "session=secret"
        assert loaded.selected_video == "https://cdn.example.test/video.m3u8?token=selected-secret"
        assert loaded.selected_audio == "https://cdn.example.test/audio.m3u8?token=audio-secret"
        assert loaded.request_contexts["https://segments.example.test"] == {
            "request_headers": {"authorization": "Bearer segments"},
            "referer": "",
            "origin": "",
            "user_agent": "",
            "cookie": "segment_session=private",
        }
        event = restored._task_event(loaded)
        assert "request_headers" not in event
        assert event["cookie"] == ""

    asyncio.run(run())


def test_browser_task_does_not_store_global_request_identity(tmp_path, monkeypatch):
    from backend.app import database as database_module

    async def run():
        monkeypatch.setattr(database_module, "DB_PATH", tmp_path / "tasks.db")
        monkeypatch.setattr(manager_module.settings, "default_referer", "https://global.test/page")
        monkeypatch.setattr(manager_module.settings, "default_origin", "https://global.test")
        monkeypatch.setattr(manager_module.settings, "default_cookie", "global=secret")
        manager = TaskManager()

        browser_task = await manager.create_task(
            "https://cdn.example.test/protected.bin",
            inherit_default_headers=False,
        )
        manual_task = await manager.create_task("https://downloads.example.test/manual.bin")

        assert browser_task.referer == ""
        assert browser_task.origin == ""
        assert browser_task.cookie == ""
        assert browser_task.engine_state["inherit_default_headers"] is False
        assert manual_task.referer == "https://global.test/page"
        assert manual_task.origin == "https://global.test"
        assert manual_task.cookie == "global=secret"

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


def test_create_task_persists_the_same_local_timestamps_shown_by_the_ui(tmp_path, monkeypatch):
    from backend.app import database as database_module

    async def run():
        monkeypatch.setattr(database_module, "DB_PATH", tmp_path / "tasks.db")
        manager = TaskManager()
        task = await manager.create_task("https://example.test/file.bin")
        rows = await database_module.run_db(
            "SELECT created_at,updated_at FROM tasks WHERE id=?", (task.id,)
        )
        assert rows[0]["created_at"] == task.created_at
        assert rows[0]["updated_at"] == task.updated_at
        assert "T" in rows[0]["created_at"]

    asyncio.run(run())


def test_load_from_db_converts_legacy_sqlite_utc_timestamp_to_local_time(tmp_path, monkeypatch):
    from datetime import datetime, timezone
    from backend.app import database as database_module

    async def run():
        monkeypatch.setattr(database_module, "DB_PATH", tmp_path / "tasks.db")
        manager = TaskManager()
        task = await manager.create_task("https://example.test/file.bin")
        await database_module.run_db(
            "UPDATE tasks SET created_at=?,updated_at=? WHERE id=?",
            ("2026-01-01 00:00:00", "2026-01-01 00:00:01", task.id),
        )
        restored = TaskManager()
        await restored.load_from_db()
        expected = datetime(2026, 1, 1, tzinfo=timezone.utc).astimezone().replace(tzinfo=None)
        assert restored.tasks[task.id].created_at == expected.isoformat()
        assert manager_module._database_timestamp("2026-01-01 00:00:01") == expected.replace(second=1).isoformat()

    asyncio.run(run())


def test_delete_terminal_task_waits_for_runner_cleanup_tail(tmp_path, monkeypatch):
    async def fake_run_db(*args, **kwargs):
        return None

    async def run():
        manager = TaskManager()
        task = _task("terminal-tail", TaskStatus.DONE)
        release_tail = asyncio.Event()

        async def finish_tail():
            await release_tail.wait()

        task.task_handle = asyncio.create_task(finish_tail())
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "temp_dir", str(tmp_path))
        monkeypatch.setattr(manager_module, "run_db", fake_run_db)

        deleting = asyncio.create_task(manager.delete_task(task.id))
        await asyncio.sleep(0)
        assert not deleting.done()
        assert task.id in manager.tasks

        release_tail.set()
        await deleting

        assert task.task_handle.done()
        assert task.id not in manager.tasks

    asyncio.run(run())


def test_concurrent_delete_requests_share_one_operation(tmp_path, monkeypatch):
    async def run():
        manager = TaskManager()
        task = _task("delete-once", TaskStatus.DONE)
        manager.tasks[task.id] = task
        database_started = asyncio.Event()
        release_database = asyncio.Event()
        database_calls = []
        events = []

        async def fake_run_db(sql, params=()):
            database_calls.append((sql, params))
            database_started.set()
            await release_database.wait()

        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "temp_dir", str(tmp_path))
        monkeypatch.setattr(manager_module, "run_db", fake_run_db)
        monkeypatch.setattr(manager, "_broadcast_nowait", lambda event: events.append(event))

        first = asyncio.create_task(manager.delete_task(task.id))
        await database_started.wait()
        second = asyncio.create_task(manager.delete_task(task.id))
        await asyncio.sleep(0)
        assert not first.done()
        assert not second.done()

        release_database.set()
        await asyncio.gather(first, second)

        assert database_calls == [("DELETE FROM tasks WHERE id=?", (task.id,))]
        assert events == [{"type": "task_deleted", "task_id": task.id}]
        assert task.id not in manager.tasks

    asyncio.run(run())


def test_delete_retries_transient_windows_file_lock(tmp_path, monkeypatch):
    async def fake_run_db(*args, **kwargs):
        return None

    async def run():
        manager = TaskManager()
        output = tmp_path / "locked.mp4"
        output.write_bytes(b"video")
        task = _task("locked-output", TaskStatus.DONE)
        task.output_path = str(output)
        manager.tasks[task.id] = task
        attempts = 0
        original_unlink = manager_module.os.unlink

        def temporarily_locked(path, *args, **kwargs):
            nonlocal attempts
            if Path(path) == output and attempts < 2:
                attempts += 1
                raise PermissionError("sharing violation")
            return original_unlink(path, *args, **kwargs)

        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "temp_dir", str(tmp_path))
        monkeypatch.setattr(manager_module, "run_db", fake_run_db)
        monkeypatch.setattr(manager_module, "DELETE_RETRY_DELAYS_SECONDS", (0, 0))
        monkeypatch.setattr(manager_module.os, "unlink", temporarily_locked)

        await manager.delete_task(task.id, delete_files=True)

        assert attempts == 2
        assert not output.exists()
        assert task.id not in manager.tasks

    asyncio.run(run())


def test_delete_keeps_task_retryable_when_file_stays_locked(tmp_path, monkeypatch):
    async def fake_run_db(*args, **kwargs):
        return None

    async def run():
        manager = TaskManager()
        output = tmp_path / "still-locked.mp4"
        output.write_bytes(b"video")
        task = _task("still-locked", TaskStatus.DONE)
        task.output_path = str(output)
        manager.tasks[task.id] = task

        def permanently_locked(_path, *args, **kwargs):
            raise PermissionError("sharing violation")

        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "temp_dir", str(tmp_path))
        monkeypatch.setattr(manager_module, "run_db", fake_run_db)
        monkeypatch.setattr(manager_module, "DELETE_RETRY_DELAYS_SECONDS", (0,))
        monkeypatch.setattr(manager_module.os, "unlink", permanently_locked)

        with pytest.raises(TaskConflictError, match="无法删除下载文件"):
            await manager.delete_task(task.id, delete_files=True)

        assert manager.tasks[task.id] is task
        assert task.id not in manager._deleting_task_ids
        assert task.id not in manager._delete_operations
        assert output.exists()

    asyncio.run(run())


@pytest.mark.skipif(os.name != "nt", reason="DPAPI migration is Windows-specific")
def test_load_from_db_migrates_all_legacy_plaintext_secret_fields(tmp_path, monkeypatch):
    from backend.app import database as database_module

    async def run():
        monkeypatch.setattr(database_module, "DB_PATH", tmp_path / "tasks.db")
        manager = TaskManager()
        task = await manager.create_task("https://cdn.example.test/legacy.m3u8")
        await database_module.run_db(
            "UPDATE tasks SET cookie=?,request_headers=?,request_contexts=?,request_method=?,request_body=?,selected_video=?,selected_audio=? WHERE id=?",
            (
                "legacy=session",
                '{"authorization":"Bearer legacy","content-type":"application/json"}',
                '{"https://segments.example.test":{"cookie":"segment=legacy"}}',
                "POST",
                "bGVnYWN5LXBvc3QtYm9keQ==",
                "https://cdn.example.test/video.m3u8?token=legacy-video",
                "https://cdn.example.test/audio.m3u8?token=legacy-audio",
                task.id,
            ),
        )

        restored = TaskManager()
        await restored.load_from_db()
        loaded = restored.tasks[task.id]
        assert loaded.cookie == "legacy=session"
        assert loaded.request_headers["authorization"] == "Bearer legacy"
        assert loaded.request_contexts["https://segments.example.test"]["cookie"] == "segment=legacy"
        assert loaded.request_body == "bGVnYWN5LXBvc3QtYm9keQ=="
        assert loaded.selected_video.endswith("legacy-video")
        assert loaded.selected_audio.endswith("legacy-audio")

        rows = await database_module.run_db(
            "SELECT cookie,request_headers,request_contexts,request_body,selected_video,selected_audio FROM tasks WHERE id=?",
            (task.id,),
        )
        assert all(str(value or "").startswith("dpapi:") for value in rows[0])

    asyncio.run(run())


def test_failed_http_checkpoint_survives_manager_cleanup_for_retry(tmp_path, monkeypatch):
    async def run():
        manager = TaskManager()
        task = _task("http-resume", TaskStatus.FAILED)
        task.engine_state["temp_dir"] = str(tmp_path)
        monkeypatch.setattr(manager_module.settings, "keep_temp_files", False)
        task_dir = manager_module.task_work_dir(task)
        task_dir.mkdir(parents=True)
        payload = task_dir / "payload.downloading"
        payload.write_bytes(b"durable-partial")
        (task_dir / "http-resume.json").write_text(
            '{"version":2,"ranges":[]}', encoding="utf-8"
        )

        await manager._cleanup_task_temp(task)

        assert payload.read_bytes() == b"durable-partial"
        assert (task_dir / "http-resume.json").is_file()

    asyncio.run(run())


def test_deleted_task_ignores_late_progress_log_and_database_callbacks(tmp_path, monkeypatch):
    database_calls = []

    async def fake_run_db(sql, params=()):
        database_calls.append((sql, params))
        return None

    async def run():
        manager = TaskManager()
        task = _task("late", TaskStatus.PAUSED)
        manager.tasks[task.id] = task
        events = []
        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module, "run_db", fake_run_db)
        monkeypatch.setattr(manager, "_broadcast_nowait", lambda event: events.append(event))

        await manager.delete_task(task.id)
        manager._on_progress(task)
        manager._on_log_write(task.id, "late worker log")
        await manager._save_db(task)

        assert [event["type"] for event in events] == ["task_deleted"]
        assert len(database_calls) == 1
        assert database_calls[0][0] == "DELETE FROM tasks WHERE id=?"
        assert not (tmp_path / ".tasks" / task.id / "download.log").exists()

    asyncio.run(run())


def test_progress_events_are_throttled_but_database_save_remains_scheduled(monkeypatch):
    async def run():
        manager = TaskManager()
        task = _task("throttled", TaskStatus.DOWNLOADING)
        manager.tasks[task.id] = task
        events = []
        monkeypatch.setattr(manager, "_broadcast_nowait", lambda event: events.append(event))
        monkeypatch.setattr(manager, "_schedule_save", lambda current: events.append({"type": "save", "id": current.id}))

        for _ in range(20):
            manager._on_progress(task)

        assert [event["type"] for event in events].count("task_progress") == 1
        assert [event["type"] for event in events].count("save") == 20

    asyncio.run(run())


def test_log_writer_uses_async_queue_and_rotates_bounded_files(tmp_path, monkeypatch):
    async def run():
        manager = TaskManager()
        task = _task("logs", TaskStatus.DOWNLOADING)
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager_module.settings, "temp_dir", str(tmp_path))
        monkeypatch.setattr(manager_module, "LOG_MAX_BYTES", 32)
        monkeypatch.setattr(manager_module, "LOG_BACKUP_COUNT", 2)

        manager._on_log_write(task.id, "first queued line")
        assert manager._log_writer_task is not None
        await manager._log_queue.put(None)
        await manager._log_writer_task

        log_path = task_work_dir(task) / "download.log"
        assert "first queued line" in log_path.read_text(encoding="utf-8")
        manager._write_log_batch([(task.id, "second line forces rotation")])
        assert log_path.with_name("download.log.1").is_file()
        assert len(list(log_path.parent.glob("download.log.*"))) <= 2

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


def test_failed_live_recording_survives_playback_cleanup(tmp_path):
    async def run():
        manager = TaskManager()
        task = Task(
            id="live-cleanup",
            url="https://example.test/live.m3u8",
            status=TaskStatus.FAILED,
        )
        task.engine_state["temp_dir"] = str(tmp_path)
        task.engine_state["live"] = True
        task_dir = manager_module.task_work_dir(task)
        seg_dir = task_dir / "segments"
        seg_dir.mkdir(parents=True, exist_ok=True)
        (seg_dir / "000000.seg").write_bytes(b"precious")
        (task_dir / "live_state.json").write_text("{}", encoding="utf-8")

        # The preview-session cleanup path must not trim a failed live
        # recording: its segments are the only copy that will ever exist.
        await manager._cleanup_task_temp(task)
        assert (seg_dir / "000000.seg").exists()
        assert (task_dir / "live_state.json").exists()

        # A failed VOD task is still trimmed as before.
        task.engine_state.pop("live")
        await manager._cleanup_task_temp(task)
        assert not seg_dir.exists()
        assert not (task_dir / "live_state.json").exists()

    asyncio.run(run())


def test_downloader_constructor_crash_cannot_leave_task_queued(monkeypatch):
    class BrokenDownloader:
        def __init__(self, *_args, **_kwargs):
            raise RuntimeError("constructor crash")

    async def run():
        manager = TaskManager()
        task = _task("constructor-crash")
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager_module, "HLSDownloader", BrokenDownloader)
        monkeypatch.setattr(manager, "_save_db", _async_noop)

        await manager.start_task(task.id)
        await task.task_handle

        assert task.status is TaskStatus.FAILED
        assert task.error_code == "DOWNLOADER_UNEXPECTED_EXIT"
        assert task.stage == "failed"

    asyncio.run(run())


def test_create_task_rolls_back_memory_when_database_insert_fails(monkeypatch):
    async def broken_db(*_args, **_kwargs):
        raise RuntimeError("database unavailable")

    async def run():
        manager = TaskManager()
        monkeypatch.setattr(manager_module, "run_db", broken_db)
        with pytest.raises(RuntimeError, match="database unavailable"):
            await manager.create_task("https://example.test/video.m3u8")
        assert manager.tasks == {}

    asyncio.run(run())


def test_load_from_db_preserves_future_scheduled_queue(monkeypatch):
    row = _db_row("queued", task_id="scheduled")
    row["engine_state"] = '{"queue_waiting_for_schedule": true}'

    async def fake_iter_db_rows(sql, params=(), **_kwargs):
        yield row

    async def run():
        manager = TaskManager()
        monkeypatch.setattr(manager_module, "iter_db_rows", fake_iter_db_rows)
        monkeypatch.setattr(manager, "_save_db", _async_noop)
        monkeypatch.setattr(manager, "_queue_auto_start_due", lambda: False)

        await manager.load_from_db()

        task = manager.tasks["scheduled"]
        assert task.status is TaskStatus.QUEUED
        assert task.engine_state["queue_waiting_for_schedule"] is True

    asyncio.run(run())


def test_load_from_db_recovers_corrupt_engine_state_and_unknown_type(monkeypatch):
    row = _db_row("paused", task_id="recovered")
    row["engine_state"] = "not-json"
    row["task_type"] = "future-protocol"
    row["url"] = "https://example.test/file.mp4"
    row["concurrency"] = "not-a-number"
    row["total_bytes"] = "broken"
    row["speed_bytes_per_sec"] = "nan"

    async def fake_iter_db_rows(sql, params=(), **_kwargs):
        yield row

    async def run():
        manager = TaskManager()
        monkeypatch.setattr(manager_module, "iter_db_rows", fake_iter_db_rows)
        await manager.load_from_db()
        task = manager.tasks["recovered"]
        assert task.task_type is TaskType.HTTP
        assert task.engine_state["state_reason"] == "database_state_recovered"
        assert task.concurrency >= 1
        assert task.progress.total_bytes == 0
        assert task.progress.speed_bytes_per_sec == 0

    asyncio.run(run())


def test_delete_database_failure_keeps_task_managed(tmp_path, monkeypatch):
    async def broken_db(*_args, **_kwargs):
        raise RuntimeError("delete failed")

    async def run():
        manager = TaskManager()
        task = _task("delete-retry", TaskStatus.PAUSED)
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module, "run_db", broken_db)

        with pytest.raises(RuntimeError, match="delete failed"):
            await manager.delete_task(task.id)

        assert manager.tasks[task.id] is task
        assert task.id not in manager._deleting_task_ids

    asyncio.run(run())


def test_temp_root_is_removed_when_every_task_is_canceled(tmp_path, monkeypatch):
    async def run():
        manager = TaskManager()
        task = _task("canceled", TaskStatus.CANCELED)
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager_module.settings, "download_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "temp_dir", str(tmp_path))
        monkeypatch.setattr(manager_module.settings, "keep_temp_files", False)
        temp_root = tmp_path / ".tasks"
        temp_root.mkdir()

        await manager._cleanup_temp_root_if_all_done()

        assert not temp_root.exists()

    asyncio.run(run())


def test_refresh_does_not_persist_the_previous_signed_url(monkeypatch):
    async def run():
        manager = TaskManager()
        task = Task(
            id="refresh-secret",
            url="https://old-edge.test/video.mp4?token=old-secret",
            task_type=TaskType.HTTP,
            status=TaskStatus.FAILED,
        )
        manager.tasks[task.id] = task
        monkeypatch.setattr(manager, "_save_db", _async_noop)

        await manager.refresh_task_request(
            task.id,
            url="https://new-edge.test/video.mp4?token=new-secret",
            auto_resume=False,
        )

        encoded = str(task.engine_state)
        assert "old-secret" not in encoded
        assert "previous_request_url" not in task.engine_state
        assert "previous_request_key" in task.engine_state

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
