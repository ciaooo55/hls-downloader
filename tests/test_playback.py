import asyncio
import json
import threading

from fastapi import FastAPI
from fastapi.testclient import TestClient

from backend.app.api import router
from backend.app.config import settings
from backend.app.downloader.playback import (
    PlaybackService,
    playback_service,
    write_playback_plan,
)
from backend.app.downloader import playback as playback_module
from backend.app.downloader.task_manager import TaskConflictError, TaskManager, manager
from backend.app.models import Task, TaskStatus, TaskType


def _segments(task_dir, durations=(4.0, 4.0, 4.0)):
    init_one = task_dir / "maps" / "0000.init"
    init_two = task_dir / "maps" / "0001.init"
    init_one.parent.mkdir(parents=True, exist_ok=True)
    init_one.write_bytes(b"init-one")
    init_two.write_bytes(b"init-two")
    return [
        {
            "index": index,
            "duration": duration,
            "discontinuity": index == 2,
            "init_path": str(init_one if index < 2 else init_two),
            "url": f"https://secret.example/{index}.m4s?token=secret",
            "key": {"uri": "https://secret.example/key", "iv": b"secret"},
        }
        for index, duration in enumerate(durations)
    ]


def test_incremental_playlist_only_exposes_contiguous_local_media(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    task_dir = tmp_path / ".tasks" / "preview1"
    segments = _segments(task_dir)
    plan_path = write_playback_plan(task_dir, segments, total_duration=12)
    plan_text = plan_path.read_text(encoding="utf-8")
    plan = json.loads(plan_text)

    assert "secret.example" not in plan_text
    assert "token=secret" not in plan_text
    assert plan["segments"][0]["init_name"] == "0000.init"

    seg_dir = task_dir / "segments"
    seg_dir.mkdir()
    (seg_dir / "000000.seg").write_bytes(b"segment-zero")
    (seg_dir / "000001.seg").write_bytes(b"segment-one")
    (seg_dir / "000002.seg.tmp").write_bytes(b"incomplete")

    service = PlaybackService()
    snapshot = service.snapshot("preview1", "downloading_segments")
    assert snapshot.ready is True
    assert snapshot.available_segments == 2
    assert snapshot.available_duration == 8

    session = service.open_session("preview1")
    playlist = service.playlist("preview1", "downloading_segments", session)
    assert "segments/000000.seg" in playlist
    assert "segments/000001.seg" in playlist
    assert "segments/000002.seg" not in playlist
    assert '#EXT-X-MAP:URI="maps/0000.init' in playlist
    assert "#EXT-X-ENDLIST" not in playlist

    tokenized = service.playlist(
        "preview1",
        "downloading_segments",
        session,
        access_token="play token",
    )
    assert "token=play%20token" in tokenized

    (seg_dir / "000002.seg").write_bytes(b"segment-two")
    completed = service.playlist("preview1", "merging", session)
    assert "#EXT-X-DISCONTINUITY" in completed
    assert '#EXT-X-MAP:URI="maps/0001.init' in completed
    assert "segments/000002.seg" in completed
    assert completed.rstrip().endswith("#EXT-X-ENDLIST")


def test_playback_plan_journal_appends_and_ignores_a_torn_tail(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    task_dir = tmp_path / ".tasks" / "journal-preview"
    segments = _segments(task_dir, durations=(2.0, 2.0))
    seg_dir = task_dir / "segments"
    seg_dir.mkdir(parents=True)
    (seg_dir / "000000.seg").write_bytes(b"first")
    (seg_dir / "000001.seg").write_bytes(b"second")

    plan_path = write_playback_plan(task_dir, segments[:1], total_duration=2.0)
    write_playback_plan(
        task_dir,
        segments,
        total_duration=4.0,
        changed_segments=[segments[1]],
    )
    journal = task_dir / "playback-plan.journal"
    assert len(json.loads(plan_path.read_text(encoding="utf-8"))["segments"]) == 1
    assert journal.exists()

    service = PlaybackService()
    assert service.snapshot("journal-preview", "downloading_segments").available_segments == 2
    with journal.open("a", encoding="utf-8") as stream:
        stream.write('{"version":1,"append":')
    # A torn final journal line cannot hide earlier durable entries.
    assert service.snapshot("journal-preview", "downloading_segments").available_segments == 2

    write_playback_plan(task_dir, segments, total_duration=4.0, force_compact=True)
    assert not journal.exists()
    assert len(json.loads(plan_path.read_text(encoding="utf-8"))["segments"]) == 2


def test_playback_plan_ignores_stale_journal_left_after_compaction_crash(
    tmp_path, monkeypatch
):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    task_dir = tmp_path / ".tasks" / "stale-journal"
    segments = _segments(task_dir, durations=(2.0, 2.0))
    seg_dir = task_dir / "segments"
    seg_dir.mkdir(parents=True)
    for index in range(2):
        (seg_dir / f"{index:06d}.seg").write_bytes(b"media")

    plan_path = write_playback_plan(task_dir, segments[:1], total_duration=2.0)
    write_playback_plan(
        task_dir,
        segments,
        total_duration=4.0,
        changed_segments=[segments[1]],
    )
    journal_path = task_dir / "playback-plan.journal"
    stale_journal = journal_path.read_bytes()
    write_playback_plan(task_dir, segments, total_duration=4.0, force_compact=True)
    # Simulate power loss after the new base was atomically published but
    # before the old journal directory entry could be removed.
    journal_path.write_bytes(stale_journal)

    base = json.loads(plan_path.read_text(encoding="utf-8"))
    event = json.loads(stale_journal.decode("utf-8").splitlines()[0])
    assert base["journal_id"] != event["journal_id"]
    service = PlaybackService()
    snapshot = service.snapshot("stale-journal", "downloading_segments")
    assert snapshot.available_segments == 2
    assert snapshot.available_duration == 4.0


def test_active_playback_waits_for_plan_compaction(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    task_dir = tmp_path / ".tasks" / "active-compaction"
    segments = _segments(task_dir, durations=(2.0, 2.0))
    seg_dir = task_dir / "segments"
    seg_dir.mkdir(parents=True)
    for index in range(2):
        (seg_dir / f"{index:06d}.seg").write_bytes(b"media")
    write_playback_plan(task_dir, segments[:1], total_duration=2.0)
    write_playback_plan(
        task_dir,
        segments,
        total_duration=4.0,
        changed_segments=[segments[1]],
    )
    service = PlaybackService()
    session = service.open_session("active-compaction")

    compact_started = threading.Event()
    allow_compact = threading.Event()
    reader_finished = threading.Event()
    real_atomic_write = playback_module.atomic_write_text

    def paused_atomic_write(path, content):
        real_atomic_write(path, content)
        compact_started.set()
        assert allow_compact.wait(timeout=5)

    monkeypatch.setattr(playback_module, "atomic_write_text", paused_atomic_write)
    writer = threading.Thread(
        target=write_playback_plan,
        args=(task_dir, segments, 4.0),
        kwargs={"force_compact": True},
    )
    writer.start()
    assert compact_started.wait(timeout=5)

    result: list[str] = []

    def read_playlist():
        result.append(service.playlist("active-compaction", "recording", session))
        reader_finished.set()

    reader = threading.Thread(target=read_playlist)
    reader.start()
    assert not reader_finished.wait(timeout=0.1)
    allow_compact.set()
    writer.join(timeout=5)
    reader.join(timeout=5)
    assert not writer.is_alive()
    assert not reader.is_alive()
    assert result[0].count("#EXTINF:") == 2


def test_first_complete_short_segment_enables_preview_only_after_its_init_map(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    task_dir = tmp_path / ".tasks" / "shortpreview"
    segments = _segments(task_dir, durations=(1.0, 1.0))
    write_playback_plan(task_dir, segments, total_duration=2.0)
    seg_dir = task_dir / "segments"
    seg_dir.mkdir()
    (seg_dir / "000000.seg").write_bytes(b"first")
    init_path = task_dir / "maps" / "0000.init"
    init_path.unlink()

    service = PlaybackService()
    # The media segment alone is not enough for fMP4 playback.
    assert service.snapshot("shortpreview", "downloading_segments").ready is False
    init_path.write_bytes(b"init")

    snapshot = service.snapshot("shortpreview", "downloading_segments")
    assert snapshot.ready is True
    _, opened = service.open_ready_session("shortpreview", "downloading_segments")
    assert opened.available_duration == 1.0


def test_full_playlist_reports_total_duration_and_seek_target(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    monkeypatch.setattr(settings, "token", "play-token")
    task = Task(
        id="seekplay",
        url="https://example.test/video.m3u8",
        status=TaskStatus.DOWNLOADING_SEGMENTS,
    )
    task_dir = tmp_path / ".tasks" / task.id
    segments = _segments(task_dir, durations=(6.0, 7.0, 8.0, 9.0))
    write_playback_plan(task_dir, segments, total_duration=30.0)
    seg_dir = task_dir / "segments"
    seg_dir.mkdir(parents=True, exist_ok=True)
    (seg_dir / "000000.seg").write_bytes(b"first")

    previous = manager.tasks
    manager.tasks = {task.id: task}
    app = FastAPI()
    app.include_router(router)
    try:
        with TestClient(app) as client:
            opened = client.post(
                f"/api/tasks/{task.id}/playback",
                headers={"X-Token": "play-token"},
            )
            assert opened.status_code == 200
            session = opened.json()["session_id"]
            playback_token = opened.json()["playback_token"]
            seek = client.post(
                f"/api/tasks/{task.id}/playback/seek",
                params={"session": session},
                headers={"X-Token": "play-token"},
                json={"time": 20},
            )
            assert seek.status_code == 200
            assert seek.json()["index"] == 2
            assert seek.json()["segment_start"] == 13

            playlist = client.get(
                f"/api/tasks/{task.id}/playback/index.m3u8",
                params={"session": session, "token": playback_token, "full": "true"},
            )
            assert playlist.status_code == 200
            assert "#EXT-X-PLAYLIST-TYPE:VOD" in playlist.text
            assert "#EXT-X-START" not in playlist.text
            assert playlist.text.rstrip().endswith("#EXT-X-ENDLIST")
            assert "segments/000003.seg" in playlist.text
            assert "segments/000002.seg" in playlist.text
            assert "full=1" in playlist.text
    finally:
        playback_service.close_task(task.id)
        manager.tasks = previous


def test_completed_media_endpoint_supports_byte_ranges(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    monkeypatch.setattr(settings, "token", "play-token")
    output = tmp_path / "video.mp4"
    output.write_bytes(bytes(range(100)))
    task = Task(
        id="doneplay",
        url="https://example.test/video.m3u8",
        status=TaskStatus.DONE,
        output_path=str(output),
    )
    previous = manager.tasks
    manager.tasks = {task.id: task}
    app = FastAPI()
    app.include_router(router)
    try:
        with TestClient(app) as client:
            opened = client.post(
                f"/api/tasks/{task.id}/playback",
                headers={"X-Token": "play-token"},
            )
            assert opened.status_code == 200
            session = opened.json()["session_id"]
            playback_token = opened.json()["playback_token"]
            unauthorized = client.get(
                f"/api/tasks/{task.id}/playback/media",
                params={"session": session, "token": "wrong"},
            )
            assert unauthorized.status_code == 401
            response = client.get(
                f"/api/tasks/{task.id}/playback/media",
                params={"session": session, "token": playback_token},
                headers={"Range": "bytes=10-19"},
            )
            assert response.status_code == 206
            assert response.headers["accept-ranges"] == "bytes"
            assert response.headers["content-range"] == "bytes 10-19/100"
            assert response.content == bytes(range(10, 20))
    finally:
        manager.tasks = previous


def test_native_hls_auth_token_is_carried_to_child_urls(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    monkeypatch.setattr(settings, "token", "play-token")
    task = Task(
        id="nativehls",
        url="https://example.test/video.m3u8",
        status=TaskStatus.DOWNLOADING_SEGMENTS,
    )
    task_dir = tmp_path / ".tasks" / task.id
    segments = _segments(task_dir, durations=(6.0,))
    write_playback_plan(task_dir, segments, total_duration=6.0)
    seg_dir = task_dir / "segments"
    seg_dir.mkdir(parents=True, exist_ok=True)
    (seg_dir / "000000.seg").write_bytes(b"segment")

    previous = manager.tasks
    manager.tasks = {task.id: task}
    app = FastAPI()
    app.include_router(router)
    try:
        with TestClient(app) as client:
            opened = client.post(
                f"/api/tasks/{task.id}/playback",
                headers={"X-Token": "play-token"},
            )
            assert opened.status_code == 200
            session = opened.json()["session_id"]
            playback_token = opened.json()["playback_token"]
            playlist = client.get(
                f"/api/tasks/{task.id}/playback/index.m3u8",
                params={"session": session, "token": playback_token},
            )
            assert playlist.status_code == 200
            assert f"token={playback_token}" in playlist.text
            segment = client.get(
                f"/api/tasks/{task.id}/playback/segments/0.seg",
                params={"session": session, "token": playback_token},
            )
            assert segment.status_code == 200
            assert segment.content == b"segment"
            denied = client.get(
                f"/api/tasks/{task.id}/playback/segments/0.seg",
                params={"session": session, "token": "wrong"},
            )
            assert denied.status_code == 401
    finally:
        playback_service.close_task(task.id)
        manager.tasks = previous


def test_native_dash_uses_local_segment_preview_before_final_mux(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path))
    monkeypatch.setattr(settings, "token", "play-token")
    task = Task(
        id="dashpreview",
        url="https://example.test/video.mpd",
        task_type=TaskType.DASH,
        status=TaskStatus.DOWNLOADING_SEGMENTS,
    )
    task.engine_state["temp_dir"] = str(tmp_path)
    task_dir = tmp_path / ".tasks" / task.id
    segments = _segments(task_dir, durations=(2.0, 2.0))
    write_playback_plan(task_dir, segments, total_duration=4.0)
    seg_dir = task_dir / "segments"
    seg_dir.mkdir(parents=True, exist_ok=True)
    (seg_dir / "000000.seg").write_bytes(b"dash-segment")

    previous = manager.tasks
    manager.tasks = {task.id: task}
    app = FastAPI()
    app.include_router(router)
    try:
        with TestClient(app) as client:
            opened = client.post(
                f"/api/tasks/{task.id}/playback",
                headers={"X-Token": "play-token"},
            )
            assert opened.status_code == 200
            assert opened.json()["mode"] == "hls"
            session = opened.json()["session_id"]
            playback_token = opened.json()["playback_token"]
            playlist = client.get(
                f"/api/tasks/{task.id}/playback/index.m3u8",
                params={"session": session, "token": playback_token},
            )
            assert playlist.status_code == 200
            assert "segments/000000.seg" in playlist.text
    finally:
        playback_service.close_task(task.id)
        manager.tasks = previous


def test_active_playback_defers_temp_cleanup_until_player_closes(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    monkeypatch.setattr(settings, "keep_temp_files", False)
    task = Task(
        id="activeplay",
        url="https://example.test/video.m3u8",
        status=TaskStatus.DONE,
        output_path=str(tmp_path / "video.mp4"),
    )
    (tmp_path / "video.mp4").write_bytes(b"final")
    task_dir = tmp_path / ".tasks" / task.id
    task_dir.mkdir(parents=True)
    (task_dir / "segment.seg").write_bytes(b"segment")

    async def run():
        local_manager = TaskManager()
        local_manager.tasks[task.id] = task
        session = playback_service.open_session(task.id)

        await local_manager._cleanup_task_temp(task)
        assert task_dir.exists()

        await local_manager.release_playback(task.id, session)
        assert not task_dir.exists()

    asyncio.run(run())


def test_paused_sparse_http_never_serves_unwritten_zero_filled_ranges(tmp_path, monkeypatch):
    async def run():
        local_manager = TaskManager()
        task = Task(
            id="paused-http",
            url="https://cdn.test/video.mp4?token=fresh",
            task_type=TaskType.HTTP,
            status=TaskStatus.PAUSED,
        )
        task.engine_state.update({
            "temp_dir": str(tmp_path),
            "stream_path": str(tmp_path / ".tasks" / task.id / "payload.downloading"),
            "total_size": 100,
        })
        task.progress.total_bytes = 100
        task.progress.downloaded_bytes = 10
        task_dir = tmp_path / ".tasks" / task.id
        task_dir.mkdir(parents=True)
        payload = task_dir / "payload.downloading"
        payload.write_bytes(b"abcdefghij" + b"\0" * 90)
        (task_dir / "http-resume.json").write_text(json.dumps({
            "version": 3,
            "resource_key": "https://cdn.test/video.mp4",
            "total": 100,
            "ranges": [{"index": 0, "from": 0, "to": 99, "current": 10}],
        }), encoding="utf-8")
        local_manager.tasks[task.id] = task

        path, total = await local_manager.wait_for_stream_range(task.id, 0, 9)
        assert path == payload
        assert total == 100
        with pytest.raises(TaskConflictError, match="尚未下载完成"):
            await local_manager.wait_for_stream_range(task.id, 10, 19)

    import pytest
    asyncio.run(run())
