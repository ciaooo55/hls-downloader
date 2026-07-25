import json

import pytest
from fastapi.testclient import TestClient
from pydantic import ValidationError

from backend.app import config as config_module
from backend.app.downloader.task_manager import TaskConflictError, TaskNotFoundError
from backend.app.main import app
from backend.app.models import Task, TaskStatus, TaskType
from backend.app.schemas import SettingsUpdate, TaskBatchCreate, TaskCreate

AUTH = {"X-Token": config_module.settings.token}


def test_task_schema_rejects_invalid_url_concurrency_and_oversized_batch():
    with pytest.raises(ValidationError):
        TaskCreate(url="ftp://example.test/video.m3u8")
    with pytest.raises(ValidationError):
        TaskCreate(url="https://example.test/video.m3u8", concurrency=257)
    with pytest.raises(ValidationError):
        TaskCreate(url="https://example.test/file.bin", checksum="sha256:bad")
    with pytest.raises(ValidationError):
        TaskBatchCreate(
            tasks=[
                TaskCreate(url=f"https://example.test/{index}.m3u8")
                for index in range(101)
            ]
        )
    with pytest.raises(ValidationError):
        SettingsUpdate(max_concurrent_tasks=0)

    assert TaskCreate(url="https://example.test/file.bin", concurrency=256).concurrency == 256
    assert TaskCreate(url="https://example.test/file.bin", checksum="A" * 64).checksum == "sha256:" + "a" * 64
    assert SettingsUpdate(default_concurrency=256).default_concurrency == 256


def test_settings_api_keeps_native_transport_credentials_internal():
    client = TestClient(app)

    response = client.get("/api/settings", headers=AUTH)
    assert response.status_code == 200
    assert {"token", "host", "port"}.isdisjoint(response.json())

    original = config_module.settings.token
    updated = client.post(
        "/api/settings",
        headers=AUTH,
        json={"token": "attacker-controlled", "host": "0.0.0.0"},
    )
    assert updated.status_code == 200
    assert config_module.settings.token == original
    assert config_module.settings.host == "127.0.0.1"
    assert {"token", "host", "port"}.isdisjoint(updated.json())

    assert client.get("/api/test").status_code == 401
    checked = client.get("/api/test", headers=AUTH)
    assert checked.status_code == 200
    assert checked.json()["browser_bridge"] == "native-messaging"


def test_torrent_upload_rejects_invalid_seed_before_creating_a_task():
    response = TestClient(app).post(
        "/api/tasks/torrent-file",
        headers=AUTH,
        files={"file": ("not-a-torrent.torrent", b"<html>blocked</html>", "application/x-bittorrent")},
    )

    assert response.status_code == 400
    assert response.json()["detail"] == "种子文件无效、已损坏，或下载到的不是 BT 种子"


def test_torrent_upload_inspects_files_and_waits_for_explicit_start(monkeypatch, tmp_path):
    """Uploading a seed must never join the swarm before the file picker confirms."""
    from backend.app import api as api_module
    from backend.app.downloader.torrent import TorrentDownloader

    created: list[Task] = []
    started: list[str] = []

    async def create_task(**_kwargs):
        task = Task(id="torrent-picker", url="torrent-file:movie.torrent", task_type=TaskType.TORRENT)
        created.append(task)
        return task

    async def save_task(_task):
        return None

    async def start_task(task_id):
        started.append(task_id)

    monkeypatch.setattr(api_module.manager, "create_task", create_task)
    monkeypatch.setattr(api_module.manager, "_save_db", save_task)
    monkeypatch.setattr(api_module.manager, "start_task", start_task)
    monkeypatch.setattr(api_module, "task_work_dir", lambda _task: tmp_path / "torrent-picker")
    monkeypatch.setattr(TorrentDownloader, "inspect_torrent_bytes", staticmethod(lambda _content: {
        "name": "movie", "piece_count": 3,
        "files": [{"index": 0, "path": "movie.mkv", "size": 42}],
    }))

    response = TestClient(app).post(
        "/api/tasks/torrent-file",
        headers=AUTH,
        files={"file": ("movie.torrent", b"torrent-bytes", "application/x-bittorrent")},
    )

    assert response.status_code == 200
    assert response.json()["status"] == "awaiting_selection"
    assert created[0].engine_state["selected_files"] == [0]
    assert started == []


def test_browser_media_push_reports_final_desktop_result(monkeypatch):
    from backend.app import api as api_module

    monkeypatch.setattr(api_module.native_desktop_session, "push", lambda *_args: True)
    client = TestClient(app)
    created = client.post(
        "/api/browser/media-push",
        headers=AUTH,
        json={"kind": "cast", "resource": {"url": "https://media.example/video.mp4", "filename": "video.mp4"}},
    )

    assert created.status_code == 200
    request_id = created.json()["id"]
    pending = client.get(f"/api/browser/media-push/{request_id}/status", headers=AUTH)
    assert pending.json()["status"] == "pending"
    completed = client.post(
        f"/api/browser/media-push/{request_id}/complete",
        headers=AUTH,
        json={"status": "canceled", "message": "用户取消"},
    )
    assert completed.status_code == 200
    final = client.get(f"/api/browser/media-push/{request_id}/status", headers=AUTH)
    assert final.json() == {"id": request_id, "status": "canceled", "message": "用户取消"}


def test_task_api_preserves_cross_origin_request_contexts(monkeypatch):
    """Manual/API clients need the same CDN authentication path as the extension."""
    from backend.app import api as api_module

    captured: list[dict] = []

    async def create_task(**kwargs):
        captured.append(kwargs)
        return Task(id=f"context-{len(captured)}", url=kwargs["url"], task_type=TaskType.HTTP)

    monkeypatch.setattr(api_module.manager, "create_task", create_task)
    payload = {
        "url": "https://manifest.example.test/master.m3u8",
        "request_contexts": {
            "https://cdn.example.test": {
                "request_headers": {"Authorization": "Bearer segment"},
                "cookie": "cdn_session=private",
            }
        },
    }
    client = TestClient(app)
    single = client.post("/api/tasks", headers=AUTH, json=payload)
    batch = client.post("/api/tasks/batch", headers=AUTH, json={"tasks": [payload]})

    assert single.status_code == 200
    assert batch.status_code == 200
    assert [item["request_contexts"] for item in captured] == [payload["request_contexts"], payload["request_contexts"]]


def test_task_action_maps_manager_errors_to_http_status(monkeypatch):
    from backend.app import api as api_module

    async def conflict(task_id):
        raise TaskConflictError("wrong state")

    async def missing(task_id):
        raise TaskNotFoundError("missing")

    client = TestClient(app)
    monkeypatch.setattr(api_module.manager, "pause_task", conflict)
    response = client.post("/api/tasks/task1/pause", headers=AUTH)
    assert response.status_code == 409
    assert response.json()["detail"] == "wrong state"

    monkeypatch.setattr(api_module.manager, "pause_task", missing)
    response = client.post("/api/tasks/task1/pause", headers=AUTH)
    assert response.status_code == 404


def test_clear_completed_only_deletes_finished_records(monkeypatch):
    from backend.app import api as api_module

    done = Task(id="done", url="https://example.test/done.m3u8", status=TaskStatus.DONE)
    failed = Task(id="failed", url="https://example.test/failed.m3u8", status=TaskStatus.FAILED)
    deleted = []

    async def delete(task_id):
        deleted.append(task_id)

    monkeypatch.setattr(api_module.manager, "tasks", {done.id: done, failed.id: failed})
    monkeypatch.setattr(api_module.manager, "delete_task", delete)

    response = TestClient(app).delete("/api/tasks/completed", headers=AUTH)

    assert response.status_code == 200
    assert response.json() == {"ok": True, "count": 1}
    assert deleted == ["done"]


def test_delete_task_can_request_output_file_removal(monkeypatch):
    from backend.app import api as api_module

    deleted = []

    async def delete(task_id, *, delete_files=False):
        deleted.append((task_id, delete_files))

    monkeypatch.setattr(api_module.manager, "delete_task", delete)
    response = TestClient(app).delete(
        "/api/tasks/task1?delete_files=true",
        headers=AUTH,
    )

    assert response.status_code == 200
    assert deleted == [("task1", True)]


def test_completed_task_file_endpoint_serves_drag_download(tmp_path, monkeypatch):
    from backend.app import api as api_module

    output = tmp_path / "setup.exe"
    output.write_bytes(b"binary")
    task = Task(
        id="drag-file",
        url="https://cdn.test/setup.exe",
        status=TaskStatus.DONE,
        output_path=str(output),
    )
    previous = api_module.manager.tasks
    monkeypatch.setattr(api_module.manager, "tasks", {task.id: task})
    try:
        response = TestClient(app).get(
            f"/api/tasks/{task.id}/file?token={config_module.settings.token}",
        )
        assert response.status_code == 200
        assert response.content == b"binary"
        assert "setup.exe" in response.headers["content-disposition"]
    finally:
        api_module.manager.tasks = previous


def test_browser_direct_download_creates_and_starts_desktop_task(monkeypatch):
    from backend.app import api as api_module

    captured = {}
    activated = []

    async def create_task(**kwargs):
        captured.update(kwargs)
        return Task(
            id="browser-task",
            url=kwargs["url"],
            title=kwargs["filename"],
            filename=kwargs["filename"],
            referer=kwargs["referer"],
            origin=kwargs["origin"],
        )

    monkeypatch.setattr(api_module.manager, "create_task", create_task)
    monkeypatch.setattr(api_module, "activate_window", lambda: activated.append(True) or True)
    response = TestClient(app).post(
        "/api/browser/downloads",
        headers=AUTH,
        json={
            "url": "https://cdn.example.test/setup.exe",
            "filename": "setup.exe",
            "source_page_url": "https://example.test/downloads",
            "referer": "https://example.test/downloads",
            "origin": "https://example.test",
            "mime_type": "application/octet-stream",
        },
    )

    assert response.status_code == 200
    assert response.json()["id"] == "browser-task"
    assert captured["auto_start"] is True
    assert captured["inherit_default_headers"] is False
    assert captured["referer"] == "https://example.test/downloads"
    assert captured["origin"] == "https://example.test"
    assert activated == [True]


def test_launch_file_requires_an_existing_file(tmp_path, monkeypatch):
    import os

    opened = []
    media = tmp_path / "video.mp4"
    media.write_bytes(b"media")
    monkeypatch.setattr(os, "startfile", lambda path: opened.append(path), raising=False)
    client = TestClient(app)

    missing = client.post("/api/launch-file", json={"path": str(tmp_path / "missing.mp4")}, headers=AUTH)
    response = client.post("/api/launch-file", json={"path": str(media)}, headers=AUTH)

    assert missing.status_code == 404
    assert response.status_code == 200
    assert opened == [str(media)]


def test_save_settings_serializes_project_paths_as_relative(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)
    settings = config_module.Settings(
        download_dir=str(config_module.PROJECT_ROOT / "downloads"),
        temp_dir=str(config_module.PROJECT_ROOT),
        ffmpeg_path=str(config_module.PROJECT_ROOT / "bin" / "ffmpeg.exe"),
    )

    config_module.save_settings(settings)

    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["download_dir"] == "downloads"
    assert saved["temp_dir"] == "."
    assert saved["ffmpeg_path"] == "bin\\ffmpeg.exe"


def test_repository_default_config_does_not_force_site_specific_request_headers():
    config_path = config_module.PROJECT_ROOT / "config.json"
    data = json.loads(config_path.read_text(encoding="utf-8"))

    assert data["config_version"] == 14
    assert data["temp_dir"] == "."
    assert data["default_referer"] == ""
    assert data["default_origin"] == ""
    assert data["default_cookie"] == ""
    assert data["default_concurrency"] == 12
    assert data["max_concurrent_tasks"] == 3


def test_settings_ignores_fields_written_by_a_future_release():
    from backend.app.config import Settings

    loaded = Settings(config_version=999, future_download_engine=True)

    assert loaded.config_version == 999
    assert not hasattr(loaded, "future_download_engine")


def test_old_blank_request_defaults_remain_blank_after_migration(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 1,
                "token": "55555",
                "download_dir": str(tmp_path / "downloads"),
                "ffmpeg_path": str(tmp_path / "ffmpeg.exe"),
                "default_referer": "",
                "default_origin": "",
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)

    loaded = config_module.load_settings()

    assert loaded.config_version == 14
    assert loaded.default_referer == ""
    assert loaded.default_origin == ""
    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["config_version"] == 14
    assert saved["token"] != "55555"
    assert len(saved["token"]) >= 32
    assert saved["temp_dir"] == "."
    assert saved["default_concurrency"] == 12
    assert saved["max_concurrent_tasks"] == 3


def test_v2_legacy_concurrency_defaults_migrate_to_new_defaults(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 2,
                "download_dir": str(tmp_path / "downloads"),
                "ffmpeg_path": str(tmp_path / "ffmpeg.exe"),
                "default_concurrency": 4,
                "max_concurrent_tasks": 2,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)

    loaded = config_module.load_settings()

    assert loaded.config_version == 14
    assert loaded.default_concurrency == 12
    assert loaded.max_concurrent_tasks == 3


def test_v2_custom_concurrency_values_are_preserved_during_migration(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 2,
                "download_dir": str(tmp_path / "downloads"),
                "ffmpeg_path": str(tmp_path / "ffmpeg.exe"),
                "default_concurrency": 6,
                "max_concurrent_tasks": 5,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)

    loaded = config_module.load_settings()

    assert loaded.config_version == 14
    assert loaded.default_concurrency == 6
    assert loaded.max_concurrent_tasks == 5


def test_v11_legacy_takeover_default_migrates_to_capture_all_explicit_downloads(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 11,
                "download_dir": str(tmp_path / "downloads"),
                "ffmpeg_path": str(tmp_path / "ffmpeg.exe"),
                "browser_takeover_min_mb": 1,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)

    loaded = config_module.load_settings()

    assert loaded.config_version == 14
    assert loaded.browser_takeover_min_mb == 0


def test_v11_custom_takeover_threshold_is_preserved(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 11,
                "download_dir": str(tmp_path / "downloads"),
                "ffmpeg_path": str(tmp_path / "ffmpeg.exe"),
                "browser_takeover_min_mb": 3,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)

    loaded = config_module.load_settings()

    assert loaded.config_version == 14
    assert loaded.browser_takeover_min_mb == 3


def test_create_browser_handoff_reports_ui_fallback(monkeypatch):
    from backend.app import api as api_module
    from backend.app import desktop_runtime as runtime
    from backend.app.browser_handoff import browser_handoffs

    runtime.register_browser_handoff(None)
    runtime.set_desktop_handoff_session(False)
    monkeypatch.setattr(api_module, "_check_token", lambda _token: None)
    monkeypatch.setattr(api_module, "_check_host", lambda _url: None)

    client = TestClient(app)
    response = client.post(
        "/api/browser/handoffs",
        json={"url": "https://cdn.example.test/video.mp4", "filename": "video.mp4"},
        headers={"X-Token": "test"},
    )
    assert response.status_code == 200
    body = response.json()
    assert body["status"] == "pending"
    assert body["presentation_mode"] == "ui-fallback"
    assert body["presentation"] == "presented"
    assert body["presented"] is True
    assert browser_handoffs.get(body["id"]).presented is True


def test_create_browser_handoff_queues_while_desktop_session_starts(monkeypatch):
    from backend.app import api as api_module
    from backend.app import desktop_runtime as runtime

    runtime.register_browser_handoff(None)
    runtime.set_desktop_handoff_session(True)
    monkeypatch.setattr(api_module, "_check_token", lambda _token: None)
    monkeypatch.setattr(api_module, "_check_host", lambda _url: None)

    client = TestClient(app)
    response = client.post(
        "/api/browser/handoffs",
        json={"url": "https://cdn.example.test/clip.mp4", "filename": "clip.mp4"},
        headers={"X-Token": "test"},
    )
    assert response.status_code == 200
    body = response.json()
    assert body["presentation_mode"] == "desktop-pending"
    assert body["presentation"] == "queued"
    assert body["presentation_queued"] is True
    runtime.set_desktop_handoff_session(False)
