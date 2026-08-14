from datetime import datetime

from backend.app.downloader.throttle import effective_download_speed_limit_kib


def _schedule(monkeypatch, **values):
    from backend.app import config as config_module

    defaults = {
        "download_speed_limit_kib": 512,
        "speed_schedule_enabled": True,
        "speed_schedule_start": "08:00",
        "speed_schedule_end": "23:00",
        "speed_schedule_limit_kib": 128,
    }
    defaults.update(values)
    current = config_module.settings.model_copy(update=defaults)
    monkeypatch.setattr(config_module, "settings", current)


def test_disabled_schedule_keeps_global_limit(monkeypatch):
    _schedule(monkeypatch, speed_schedule_enabled=False)
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 12, 0)) == 512


def test_inside_window_uses_scheduled_limit(monkeypatch):
    _schedule(monkeypatch)
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 8, 0)) == 128
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 12, 30)) == 128


def test_window_end_is_exclusive(monkeypatch):
    _schedule(monkeypatch)
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 23, 0)) == 512
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 7, 59)) == 512


def test_outside_window_uses_global_limit(monkeypatch):
    _schedule(monkeypatch)
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 23, 30)) == 512


def test_overnight_window_wraps_midnight(monkeypatch):
    _schedule(monkeypatch, speed_schedule_start="22:00", speed_schedule_end="08:00", speed_schedule_limit_kib=64)
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 22, 0)) == 64
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 23, 15)) == 64
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 7, 59)) == 64
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 8, 0)) == 512
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 12, 0)) == 512


def test_scheduled_zero_means_unlimited_in_window(monkeypatch):
    _schedule(monkeypatch, speed_schedule_limit_kib=0)
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 10, 0)) == 0
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 23, 30)) == 512


def test_bad_or_equal_times_fall_back_to_global(monkeypatch):
    _schedule(monkeypatch, speed_schedule_start="25:00", speed_schedule_end="23:00")
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 12, 0)) == 512
    _schedule(monkeypatch, speed_schedule_start="08:00", speed_schedule_end="xx:yy")
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 12, 0)) == 512
    _schedule(monkeypatch, speed_schedule_start="08:00", speed_schedule_end="08:00")
    assert effective_download_speed_limit_kib(datetime(2026, 8, 13, 8, 0)) == 512


def test_settings_schedule_defaults_are_off():
    from backend.app.config import Settings

    loaded = Settings()
    assert loaded.speed_schedule_enabled is False
    assert loaded.speed_schedule_start == "08:00"
    assert loaded.speed_schedule_end == "23:00"
    assert loaded.speed_schedule_limit_kib == 0
    assert loaded.download_speed_limit_kib == 0


class _FrozenDateTime(datetime):
    @classmethod
    def now(cls, tz=None):
        return cls(2026, 8, 13, 12, 0, 0)


def test_public_settings_expose_effective_limit(monkeypatch):
    from backend.app import api as api_module
    from backend.app import config as config_module
    from backend.app.downloader import throttle as throttle_module

    _schedule(monkeypatch)
    monkeypatch.setattr(api_module, "settings", config_module.settings)
    monkeypatch.setattr(throttle_module, "datetime", _FrozenDateTime)
    body = api_module._public_settings()
    assert body["download_speed_limit_kib"] == 512
    assert body["speed_schedule_enabled"] is True
    assert body["effective_download_speed_limit_kib"] == 128


def test_v23_config_gains_schedule_defaults(tmp_path, monkeypatch):
    import json

    from backend.app import config as config_module

    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 23,
                "token": "x" * 40,
                "download_dir": str(tmp_path / "downloads"),
                "ffmpeg_path": str(tmp_path / "ffmpeg.exe"),
                "download_speed_limit_kib": 256,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)
    loaded = config_module.load_settings()
    assert loaded.config_version == 28
    assert loaded.download_speed_limit_kib == 256
    assert loaded.speed_schedule_enabled is False
    assert loaded.speed_schedule_start == "08:00"
    assert loaded.speed_schedule_end == "23:00"
    assert loaded.speed_schedule_limit_kib == 0
    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["config_version"] == 28
    assert saved["speed_schedule_enabled"] is False
