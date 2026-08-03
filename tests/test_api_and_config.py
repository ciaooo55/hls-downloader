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
        TaskCreate(url="https://example.test/video.m3u8", concurrency=65)
    with pytest.raises(ValidationError):
        TaskCreate(url="https://example.test/file.bin", checksum="sha256:bad")
    with pytest.raises(ValidationError):
        TaskCreate(
            url="https://example.test/file.bin",
            request_headers={f"x-{index}": "value" for index in range(65)},
        )
    with pytest.raises(ValidationError):
        TaskCreate(
            url="https://example.test/file.bin",
            request_contexts={f"https://cdn-{index}.test": {} for index in range(13)},
        )
    with pytest.raises(ValidationError):
        TaskBatchCreate(
            tasks=[
                TaskCreate(url=f"https://example.test/{index}.m3u8")
                for index in range(101)
            ]
        )


def test_global_json_body_limit_rejects_oversized_payload():
    response = TestClient(app).post(
        "/api/tasks",
        headers={**AUTH, "Content-Type": "application/json"},
        content=b"{" + b" " * (4 * 1024 * 1024) + b"}",
    )

    assert response.status_code == 413
    with pytest.raises(ValidationError):
        SettingsUpdate(max_concurrent_tasks=0)

    assert TaskCreate(url="https://example.test/file.bin", concurrency=64).concurrency == 64
    assert TaskCreate(url="https://example.test/file.bin", checksum="A" * 64).checksum == "sha256:" + "a" * 64
    assert SettingsUpdate(default_concurrency=64).default_concurrency == 64


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


def test_torrent_upload_bounds_multipart_title_and_request(monkeypatch):
    from backend.app import api as api_module

    monkeypatch.setattr(api_module, "_check_token", lambda _token: None)
    client = TestClient(app)
    oversized_title = client.post(
        "/api/tasks/torrent-file",
        files={"file": ("sample.torrent", b"x", "application/x-bittorrent")},
        data={"title": "x" * 513},
        headers=AUTH,
    )
    oversized_request = client.build_request(
        "POST",
        "/api/tasks/torrent-file",
        files={"file": ("sample.torrent", b"x", "application/x-bittorrent")},
        data={"title": "ok"},
        headers=AUTH,
    )
    oversized_request.headers["content-length"] = str(api_module.MAX_TORRENT_MULTIPART_BODY_BYTES + 1)
    oversized_response = client.send(oversized_request)

    assert oversized_title.status_code == 422
    assert oversized_response.status_code == 413


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


def test_recognize_uses_explicit_manual_request_context(monkeypatch):
    """Manual recognition must not drop an auth header before task creation."""
    from backend.app import api as api_module

    captured: dict[str, object] = {}

    async def recognize(url, headers, client=None):
        captured["url"] = url
        captured["headers"] = headers
        return {"status": "ready", "candidates": []}

    monkeypatch.setattr(api_module, "recognize_url", recognize)
    response = TestClient(app).post(
        "/api/recognize",
        headers=AUTH,
        json={
            "url": "https://cdn.example.test/master.m3u8",
            "referer": "https://site.example.test/watch/42",
            "origin": "https://site.example.test",
            "cookie": "session=private",
            "request_headers": {
                "Authorization": "Bearer media",
                "X-Playback-Token": "token",
                "Host": "must-not-pass.example.test",
            },
        },
    )

    assert response.status_code == 200
    assert captured["url"] == "https://cdn.example.test/master.m3u8"
    assert captured["headers"] == {
        "authorization": "Bearer media",
        "x-playback-token": "token",
        "user-agent": config_module.settings.default_user_agent,
        "referer": "https://site.example.test/watch/42",
        "origin": "https://site.example.test",
        "cookie": "session=private",
    }


def test_task_response_exposes_safe_request_method_but_never_replay_body():
    from backend.app import api as api_module

    response = api_module._to_resp(Task(
        id="post-task", url="https://api.example.test/export", task_type=TaskType.HTTP,
        request_method="POST", request_body="private-base64-body",
    )).model_dump()

    assert response["request_method"] == "POST"
    assert "request_body" not in response


def test_task_response_preserves_schedule_and_completion_action():
    from backend.app import api as api_module

    task = Task(id="scheduled", url="https://cdn.example.test/file.zip", task_type=TaskType.HTTP)
    task.engine_state.update({
        "scheduled_start_at": "2026-08-03T08:00:00+08:00",
        "scheduled_stop_at": "2026-08-03T09:00:00+08:00",
        "completion_action": "sleep",
    })

    response = api_module._to_resp(task).model_dump()

    assert response["scheduled_start_at"] == "2026-08-03T08:00:00+08:00"
    assert response["scheduled_stop_at"] == "2026-08-03T09:00:00+08:00"
    assert response["completion_action"] == "sleep"


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
        safe = api_module._to_resp(task)
        response = TestClient(app).get(
            f"/api/tasks/{task.id}/file?token={safe.file_access_token}",
        )
        assert response.status_code == 200
        control_token_in_url = TestClient(app).get(
            f"/api/tasks/{task.id}/file?token={config_module.settings.token}",
        )
        assert control_token_in_url.status_code == 401
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


def test_launch_file_requires_a_completed_task_output(tmp_path, monkeypatch):
    from backend.app import api as api_module
    import os

    opened = []
    media = tmp_path / "video.mp4"
    media.write_bytes(b"media")
    task = Task(
        id="launch-media",
        url="https://cdn.test/video.mp4",
        task_type=TaskType.HTTP,
        status=TaskStatus.DONE,
        output_path=str(media),
    )
    api_module.manager.tasks[task.id] = task
    monkeypatch.setattr(os, "startfile", lambda path: opened.append(path), raising=False)
    client = TestClient(app)

    try:
        arbitrary = client.post(
            "/api/launch-file", json={"path": str(media)}, headers=AUTH
        )
        missing = client.post(
            "/api/launch-file", json={"task_id": "missing"}, headers=AUTH
        )
        response = client.post(
            "/api/launch-file", json={"task_id": task.id}, headers=AUTH
        )
    finally:
        api_module.manager.tasks.pop(task.id, None)

    assert arbitrary.status_code == 400
    assert missing.status_code == 404
    assert response.status_code == 200
    assert opened == [str(media)]


def test_launch_executable_requires_explicit_confirmation(tmp_path, monkeypatch):
    from backend.app import api as api_module
    import os

    opened = []
    executable = tmp_path / "setup.exe"
    executable.write_bytes(b"MZ")
    task = Task(
        id="launch-executable",
        url="https://cdn.test/setup.exe",
        task_type=TaskType.HTTP,
        status=TaskStatus.DONE,
        output_path=str(executable),
    )
    api_module.manager.tasks[task.id] = task
    monkeypatch.setattr(os, "startfile", lambda path: opened.append(path), raising=False)
    client = TestClient(app)

    try:
        blocked = client.post(
            "/api/launch-file", json={"task_id": task.id}, headers=AUTH
        )
        confirmed = client.post(
            "/api/launch-file",
            json={"task_id": task.id, "confirm_executable": True},
            headers=AUTH,
        )
    finally:
        api_module.manager.tasks.pop(task.id, None)

    assert blocked.status_code == 409
    assert confirmed.status_code == 200
    assert opened == [str(executable)]


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


def test_config_credentials_are_dpapi_protected_and_restore_at_runtime(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)
    settings = config_module.Settings(
        default_cookie="session=config-secret",
        proxy_mode="manual",
        proxy_url="http://proxy-user:proxy-pass@127.0.0.1:8080",
        site_profiles=[{
            "host": "example.test",
            "request_headers": {
                "Authorization": "Bearer site-secret",
                "X-Public-Mode": "video",
            },
        }],
    )

    config_module.save_settings(settings)
    stored_text = config_path.read_text(encoding="utf-8")
    stored = json.loads(stored_text)
    assert "config-secret" not in stored_text
    assert "proxy-pass" not in stored_text
    assert "site-secret" not in stored_text
    assert stored["default_cookie"].startswith("dpapi:")
    assert stored["proxy_url"].startswith("dpapi:")
    assert stored["site_profiles"][0]["request_headers"]["Authorization"].startswith("dpapi:")
    assert stored["site_profiles"][0]["request_headers"]["X-Public-Mode"] == "video"

    restored = config_module.load_settings()
    assert restored.default_cookie == "session=config-secret"
    assert restored.proxy_url == "http://proxy-user:proxy-pass@127.0.0.1:8080"
    assert restored.site_profiles[0]["request_headers"]["Authorization"] == "Bearer site-secret"


def test_settings_api_masks_credentials_and_preserves_masked_updates(tmp_path, monkeypatch):
    from backend.app import api as api_module

    monkeypatch.setattr(config_module, "CONFIG_PATH", tmp_path / "config.json")
    monkeypatch.setattr(api_module, "_check_token", lambda _token: None)
    monkeypatch.setattr(api_module.settings, "default_cookie", "session=api-secret")
    monkeypatch.setattr(api_module.settings, "proxy_mode", "manual")
    monkeypatch.setattr(api_module.settings, "proxy_url", "http://user:pass@127.0.0.1:8080")
    monkeypatch.setattr(api_module.settings, "site_profiles", [{
        "host": "example.test",
        "request_headers": {"Authorization": "Bearer secret", "X-Mode": "video"},
    }])

    public = api_module._public_settings()
    serialized = json.dumps(public, ensure_ascii=False)
    assert "api-secret" not in serialized
    assert "user:pass" not in serialized
    assert "Bearer secret" not in serialized
    assert public["default_cookie"] == ""
    assert public["default_cookie_configured"] is True
    assert public["proxy_url"] == "••••••••"
    assert public["proxy_url_configured"] is True
    assert public["site_profiles"][0]["request_headers"]["Authorization"] == "••••••••"

    client = TestClient(app)
    response = client.post(
        "/api/settings",
        json={
            "default_cookie": "••••••••",
            "proxy_url": "••••••••",
            "site_profiles": public["site_profiles"],
        },
        headers={"X-Token": "test"},
    )
    assert response.status_code == 200
    assert api_module.settings.default_cookie == "session=api-secret"
    assert api_module.settings.proxy_url == "http://user:pass@127.0.0.1:8080"
    assert api_module.settings.site_profiles[0]["request_headers"]["Authorization"] == "Bearer secret"


def test_distributable_default_config_does_not_force_site_specific_request_headers():
    config_path = config_module.PROJECT_ROOT / "config.default.json"
    data = json.loads(config_path.read_text(encoding="utf-8"))

    # The checked-in template must not ship a reusable privileged credential.
    # The release template already uses the current credential-protection schema.
    assert data["config_version"] == 20
    assert "token" not in data
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

    assert loaded.config_version == 20
    assert loaded.default_referer == ""
    assert loaded.default_origin == ""
    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["config_version"] == 20
    assert saved["token"] != "55555"
    assert len(saved["token"]) >= 32
    assert saved["temp_dir"] == "."
    assert saved["default_concurrency"] == 12
    assert saved["max_concurrent_tasks"] == 3


def test_current_config_can_never_rebind_internal_api_to_lan(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    config_path.write_text(json.dumps({
        "config_version": 18,
        "host": "0.0.0.0",
        "token": "x" * 40,
        "download_dir": str(tmp_path / "downloads"),
        "temp_dir": str(tmp_path / "temp"),
    }), encoding="utf-8")
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)

    loaded = config_module.load_settings()

    assert loaded.host == "127.0.0.1"
    assert json.loads(config_path.read_text(encoding="utf-8"))["host"] == "127.0.0.1"


def test_corrupt_config_is_preserved_and_replaced_atomically(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    config_path.write_text('{"config_version":', encoding="utf-8")
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)

    loaded = config_module.load_settings()

    assert loaded.host == "127.0.0.1"
    assert json.loads(config_path.read_text(encoding="utf-8"))["token"]
    backups = list(tmp_path.glob("config.json.corrupt-*"))
    assert len(backups) == 1
    assert backups[0].read_text(encoding="utf-8") == '{"config_version":'
    assert not (tmp_path / "config.json.tmp").exists()


def test_publicly_leaked_token_is_rotated_on_load(tmp_path, monkeypatch):
    # config.json used to be git-tracked, so this token reached a public
    # commit; it must never authenticate an installation again.
    leaked = "ktHjYK8MXbRKgH0QtuGQl1n4duHVHAMECEbOpiTNCqM"
    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps({"config_version": 14, "token": leaked}), encoding="utf-8"
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)

    loaded = config_module.load_settings()

    assert loaded.token != leaked
    assert len(loaded.token) >= 32
    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["token"] == loaded.token
    assert saved["config_version"] == 20


def test_runtime_config_is_not_tracked_by_git():
    """config.json holds the per-install IPC token and must stay untracked."""
    import subprocess

    tracked = subprocess.run(
        ["git", "ls-files", "config.json"],
        cwd=str(config_module.PROJECT_ROOT),
        capture_output=True,
        text=True,
    ).stdout.strip()
    assert tracked == "", "config.json must never be tracked: it holds a secret"
    ignore_rules = (config_module.PROJECT_ROOT / ".gitignore").read_text(encoding="utf-8")
    assert "config.json" in ignore_rules


def test_v13_template_generates_a_per_install_native_transport_token(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 13,
                "download_dir": str(tmp_path / "downloads"),
                "ffmpeg_path": str(tmp_path / "ffmpeg.exe"),
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)

    loaded = config_module.load_settings()
    saved = json.loads(config_path.read_text(encoding="utf-8"))

    assert loaded.config_version == 20
    assert saved["config_version"] == 20
    assert len(saved["token"]) >= 32


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

    assert loaded.config_version == 20
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

    assert loaded.config_version == 20
    assert loaded.default_concurrency == 6
    assert loaded.max_concurrent_tasks == 5


def test_v19_excessive_worker_counts_are_clamped_to_global_budget(tmp_path, monkeypatch):
    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 19,
                "download_dir": str(tmp_path / "downloads"),
                "temp_dir": str(tmp_path / "cache"),
                "ffmpeg_path": str(tmp_path / "ffmpeg.exe"),
                "default_concurrency": 256,
                "site_profiles": [
                    {"host": "example.test", "concurrency": 256},
                ],
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)

    loaded = config_module.load_settings()

    assert loaded.config_version == 20
    assert loaded.default_concurrency == 64
    assert loaded.site_profiles[0]["concurrency"] == 64


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

    assert loaded.config_version == 20
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

    assert loaded.config_version == 20
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


def test_browser_handoff_rejects_oversized_body_and_fields(monkeypatch):
    from backend.app import api as api_module

    monkeypatch.setattr(api_module, "_check_token", lambda _token: None)
    client = TestClient(app)

    oversized_body = client.post(
        "/api/browser/handoffs",
        content=b"{" + b" " * (api_module.MAX_BROWSER_JSON_BODY_BYTES + 1),
        headers={"X-Token": "test", "Content-Type": "application/json"},
    )
    oversized_title = client.post(
        "/api/browser/handoffs",
        json={"url": "https://cdn.example.test/video.mp4", "title": "x" * 513},
        headers={"X-Token": "test"},
    )
    too_many_contexts = client.post(
        "/api/browser/handoffs",
        json={
            "url": "https://cdn.example.test/video.mp4",
            "request_contexts": {
                f"https://cdn-{index}.example.test": {} for index in range(13)
            },
        },
        headers={"X-Token": "test"},
    )

    assert oversized_body.status_code == 413
    assert oversized_title.status_code == 422
    assert too_many_contexts.status_code == 422


def test_standalone_ui_credential_bootstrap_requires_exact_loopback_origin(monkeypatch):
    from backend.app import api as api_module

    monkeypatch.setattr(api_module.settings, "port", 8765)
    client = TestClient(app)

    allowed = client.post(
        "/api/ui/credential",
        json={},
        headers={"Origin": "http://127.0.0.1:8765"},
    )
    rejected = client.post(
        "/api/ui/credential",
        json={},
        headers={"Origin": "https://evil.example"},
    )
    missing = client.post("/api/ui/credential", json={})

    assert allowed.status_code == 200
    assert allowed.json()["credential"].startswith("desktop.")
    assert rejected.status_code == 403
    assert missing.status_code == 403


def test_browser_handoff_sanitizes_contexts_before_memory_storage(monkeypatch):
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
        json={
            "url": "https://cdn.example.test/video.mp4",
            "request_contexts": {
                "not-an-origin": {"cookie": "discard=1"},
                "https://segments.example.test/path": {
                    "cookie": "x" * (20 * 1024),
                    "request_headers": {"Host": "evil.test", "X-Safe": "yes"},
                },
            },
        },
        headers={"X-Token": "test"},
    )

    assert response.status_code == 200
    item = browser_handoffs.get(response.json()["id"])
    assert set(item.request_contexts) == {"https://segments.example.test"}
    assert len(item.request_contexts["https://segments.example.test"]["cookie"]) == 16 * 1024
    assert item.request_contexts["https://segments.example.test"]["request_headers"] == {"x-safe": "yes"}


def test_cancel_browser_handoff_can_suppress_one_site_resource_kind(monkeypatch):
    from backend.app import api as api_module
    from backend.app import desktop_runtime as runtime

    runtime.register_browser_handoff(None)
    runtime.set_desktop_handoff_session(False)
    monkeypatch.setattr(api_module, "_check_token", lambda _token: None)
    monkeypatch.setattr(api_module, "_check_host", lambda _url: None)

    client = TestClient(app)
    created = client.post(
        "/api/browser/handoffs",
        json={
            "url": "https://cdn.example.test/video.m3u8",
            "source_page_url": "https://watch.example.test/episode/42",
            "resource_kind": "hls",
        },
        headers={"X-Token": "test"},
    ).json()
    canceled = client.post(
        f"/api/browser/handoffs/{created['id']}/cancel",
        json={"suppress_site_kind": True},
        headers={"X-Token": "test"},
    )

    assert canceled.status_code == 200
    assert canceled.json()["status"] == "canceled"
    assert canceled.json()["suppression"] == {
        "host": "watch.example.test",
        "kind": "hls",
    }


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


def test_browser_handoff_manual_context_overrides_are_scoped_to_download_origin(tmp_path, monkeypatch):
    """A user-entered 403 workaround must reach a cross-origin media URL."""
    from backend.app import api as api_module
    from backend.app import desktop_runtime as runtime
    from backend.app.models import Task

    runtime.register_browser_handoff(None)
    runtime.set_desktop_handoff_session(False)
    monkeypatch.setattr(api_module, "_check_token", lambda _token: None)
    monkeypatch.setattr(api_module, "_check_host", lambda _url: None)

    captured = {}
    async def create_task(item, output_dir=""):
        captured["item"] = item
        return Task(id="manual-context", url=item.url)

    monkeypatch.setattr(api_module, "_create_browser_task", create_task)
    client = TestClient(app)
    created = client.post(
        "/api/browser/handoffs",
        json={
            "url": "https://cdn.example.test/video.m3u8",
            "source_page_url": "https://site.example.test/watch/42",
            "cookie": "page=default",
            "request_headers": {"referer": "https://site.example.test/watch/42"},
        },
        headers={"X-Token": "test"},
    ).json()
    accepted = client.post(
        f"/api/browser/handoffs/{created['id']}/accept",
        json={
            "download_dir": str(tmp_path),
            "cookie": "manual=secret",
            "request_headers": {
                "Authorization": "Bearer manual",
                "Origin": "https://manual.example.test",
                "Referer": "https://manual.example.test/watch",
                "X-Token": "x",
            },
        },
        headers={"X-Token": "test"},
    )
    assert accepted.status_code == 200
    item = captured["item"]
    context = item.request_contexts["https://cdn.example.test"]
    assert context["cookie"] == "manual=secret"
    assert context["origin"] == "https://manual.example.test"
    assert context["referer"] == "https://manual.example.test/watch"
    assert context["request_headers"]["authorization"] == "Bearer manual"
    assert context["request_headers"]["x-token"] == "x"
    detail = client.get(
        f"/api/browser/handoffs/{created['id']}",
        headers={"X-Token": "test"},
    )
    assert detail.status_code == 200
    assert detail.json()["effective_context"]["target_origin"] == "https://cdn.example.test"
    assert detail.json()["effective_context"]["cookie"] == "manual=secret"
    assert detail.json()["effective_context"]["request_headers"]["referer"] == "https://manual.example.test/watch"
    runtime.set_desktop_handoff_session(False)


def test_directory_browser_is_bounded_to_configured_roots(tmp_path, monkeypatch):
    from types import SimpleNamespace
    from backend.app import api as api_module

    download = tmp_path / "downloads"
    temporary = tmp_path / "cache"
    data = tmp_path / "data"
    outside = tmp_path / "private"
    for path in (download, temporary, data, outside):
        path.mkdir()
    (download / "child").mkdir()
    monkeypatch.setattr(config_module.settings, "download_dir", str(download))
    monkeypatch.setattr(config_module.settings, "temp_dir", str(temporary))
    monkeypatch.setattr(api_module, "RUNTIME_PATHS", SimpleNamespace(data_root=data))
    monkeypatch.setattr(api_module.manager, "tasks", {})

    with TestClient(app) as client:
        allowed = client.get(
            "/api/browse-dir",
            params={"path": str(download), "limit": 1},
            headers=AUTH,
        )
        blocked = client.get(
            "/api/browse-dir",
            params={"path": str(outside)},
            headers=AUTH,
        )

    assert allowed.status_code == 200
    assert allowed.json()["limit"] == 1
    assert allowed.json()["items"][0]["name"] == "child"
    assert blocked.status_code == 403
