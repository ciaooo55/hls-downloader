import json

import pytest
from fastapi.testclient import TestClient
from pydantic import ValidationError

from backend.app import config as config_module
from backend.app.downloader.task_manager import TaskConflictError, TaskNotFoundError
from backend.app.main import app
from backend.app.schemas import SettingsUpdate, TaskBatchCreate, TaskCreate


def test_task_schema_rejects_invalid_url_concurrency_and_oversized_batch():
    with pytest.raises(ValidationError):
        TaskCreate(url="ftp://example.test/video.m3u8")
    with pytest.raises(ValidationError):
        TaskCreate(url="https://example.test/video.m3u8", concurrency=65)
    with pytest.raises(ValidationError):
        TaskBatchCreate(
            tasks=[
                TaskCreate(url=f"https://example.test/{index}.m3u8")
                for index in range(101)
            ]
        )
    with pytest.raises(ValidationError):
        SettingsUpdate(max_concurrent_tasks=0)


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


def test_save_settings_serializes_project_paths_as_relative(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)
    settings = config_module.Settings(
        download_dir=str(config_module.PROJECT_ROOT / "downloads"),
        ffmpeg_path=str(config_module.PROJECT_ROOT / "bin" / "ffmpeg.exe"),
    )

    config_module.save_settings(settings)

    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["download_dir"] == "downloads"
    assert saved["ffmpeg_path"] == "bin\\ffmpeg.exe"


def test_repository_default_config_uses_missav_request_headers():
    config_path = config_module.PROJECT_ROOT / "config.json"
    data = json.loads(config_path.read_text(encoding="utf-8"))

    assert data["config_version"] == 3
    assert data["default_referer"] == "https://missav.ai/"
    assert data["default_origin"] == "https://missav.ai"
    assert data["default_cookie"] == ""
    assert data["default_concurrency"] == 8
    assert data["max_concurrent_tasks"] == 3


def test_old_blank_request_defaults_migrate_to_missav(tmp_path, monkeypatch):
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

    assert loaded.config_version == 3
    assert loaded.default_referer == "https://missav.ai/"
    assert loaded.default_origin == "https://missav.ai"
    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["config_version"] == 3
    assert saved["default_concurrency"] == 8
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

    assert loaded.config_version == 3
    assert loaded.default_concurrency == 8
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

    assert loaded.config_version == 3
    assert loaded.default_concurrency == 6
    assert loaded.max_concurrent_tasks == 5
