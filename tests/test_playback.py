import asyncio
import json

from fastapi import FastAPI
from fastapi.testclient import TestClient

from backend.app.api import router
from backend.app.config import settings
from backend.app.downloader.playback import (
    PlaybackService,
    playback_service,
    write_playback_plan,
)
from backend.app.downloader.task_manager import TaskManager, manager
from backend.app.models import Task, TaskStatus


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
                params={"session": session, "token": "play-token", "full": "true"},
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
            unauthorized = client.get(
                f"/api/tasks/{task.id}/playback/media",
                params={"session": session, "token": "wrong"},
            )
            assert unauthorized.status_code == 401
            response = client.get(
                f"/api/tasks/{task.id}/playback/media",
                params={"session": session, "token": "play-token"},
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
            playlist = client.get(
                f"/api/tasks/{task.id}/playback/index.m3u8",
                params={"session": session, "token": "play-token"},
            )
            assert playlist.status_code == 200
            assert "token=play-token" in playlist.text
            segment = client.get(
                f"/api/tasks/{task.id}/playback/segments/0.seg",
                params={"session": session, "token": "play-token"},
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
