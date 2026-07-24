import json

import pytest
from fastapi.testclient import TestClient
from pydantic import ValidationError

from backend.app import config as config_module
from backend.app.downloader.task_manager import TaskConflictError, TaskNotFoundError
from backend.app.main import app
from backend.app.models import Task, TaskStatus
from backend.app.schemas import SettingsUpdate, TaskBatchCreate, TaskCreate


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


def test_task_action_maps_manager_errors_to_http_status(monkeypatch):
    from backend.app import api as api_module

    async def conflict(task_id):
        raise TaskConflictError("wrong state")

    async def missing(task_id):
        raise TaskNotFoundError("missing")

    client = TestClient(app)
    monkeypatch.setattr(api_module.manager, "pause_task", conflict)
    response = client.post("/api/tasks/task1/pause", headers={"X-Token": "55555"})
    assert response.status_code == 409
    assert response.json()["detail"] == "wrong state"

    monkeypatch.setattr(api_module.manager, "pause_task", missing)
    response = client.post("/api/tasks/task1/pause", headers={"X-Token": "55555"})
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

    response = TestClient(app).delete("/api/tasks/completed", headers={"X-Token": "55555"})

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
        headers={"X-Token": "55555"},
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
            f"/api/tasks/{task.id}/file?token=55555",
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
        headers={"X-Token": "55555"},
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

    missing = client.post("/api/launch-file", json={"path": str(tmp_path / "missing.mp4")}, headers={"X-Token": "55555"})
    response = client.post("/api/launch-file", json={"path": str(media)}, headers={"X-Token": "55555"})

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

    assert data["config_version"] == 11
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

    assert loaded.config_version == 11
    assert loaded.default_referer == ""
    assert loaded.default_origin == ""
    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["config_version"] == 11
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

    assert loaded.config_version == 11
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

    assert loaded.config_version == 11
    assert loaded.default_concurrency == 6
    assert loaded.max_concurrent_tasks == 5


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
