import asyncio
import json

from backend.app.config import Settings
from backend.app.downloader.task_manager import TaskManager
from backend.app.models import TaskStatus
from backend.app.downloader import task_manager as manager_module
from tests.test_task_manager_lifecycle import _async_noop, _db_row


def test_resume_interrupted_defaults_off():
    loaded = Settings()
    assert loaded.resume_interrupted_on_startup is False
    assert loaded.config_version == 28


def test_v25_config_gains_opt_in_startup_resume(tmp_path, monkeypatch):
    from backend.app import config as config_module

    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 25,
                "token": "x" * 40,
                "download_dir": str(tmp_path / "downloads"),
                "ffmpeg_path": str(tmp_path / "ffmpeg.exe"),
                "completion_sound_enabled": True,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)
    loaded = config_module.load_settings()
    assert loaded.config_version == 28
    assert loaded.resume_interrupted_on_startup is False
    assert loaded.completion_sound_enabled is True
    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["config_version"] == 28
    assert saved["resume_interrupted_on_startup"] is False


def test_load_from_db_auto_resumes_interrupted_when_enabled(monkeypatch):
    settings = manager_module.settings
    monkeypatch.setattr(settings, "resume_interrupted_on_startup", True)
    monkeypatch.setattr(settings, "queue_auto_start_enabled", False)

    async def fake_iter_db_rows(sql, params=(), **_kwargs):
        yield _db_row("downloading_segments", task_id="crashed")
        yield _db_row("paused", task_id="manual-paused")

    async def run():
        manager = TaskManager()
        started = []

        async def fake_start(task_id):
            started.append(task_id)

        monkeypatch.setattr(manager_module, "iter_db_rows", fake_iter_db_rows)
        monkeypatch.setattr(manager, "_save_db", _async_noop)
        monkeypatch.setattr(manager, "start_task", fake_start)
        await manager.load_from_db()
        assert started == ["crashed"]
        assert manager.tasks["crashed"].engine_state["state_reason"] == "startup_resume"
        assert manager.tasks["manual-paused"].status is TaskStatus.PAUSED
        assert manager.tasks["manual-paused"].stage != "interrupted"

    asyncio.run(run())


def test_load_from_db_skips_startup_resume_before_legal_acceptance(monkeypatch):
    settings = manager_module.settings
    monkeypatch.setattr(settings, "resume_interrupted_on_startup", True)

    async def fake_iter_db_rows(sql, params=(), **_kwargs):
        yield _db_row("downloading", task_id="gated")

    async def run():
        manager = TaskManager()
        started = []

        async def fake_start(task_id):
            started.append(task_id)

        monkeypatch.setattr(manager_module, "iter_db_rows", fake_iter_db_rows)
        monkeypatch.setattr(manager, "_save_db", _async_noop)
        monkeypatch.setattr(manager, "start_task", fake_start)
        await manager.load_from_db(auto_start_allowed=False)
        assert started == []
        assert manager.tasks["gated"].status is TaskStatus.PAUSED
        assert manager.tasks["gated"].stage == "interrupted"

    asyncio.run(run())
