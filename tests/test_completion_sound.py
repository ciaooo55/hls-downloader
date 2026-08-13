from backend.app.config import Settings


def test_completion_sound_defaults_off():
    loaded = Settings()
    assert loaded.completion_sound_enabled is False
    assert loaded.config_version == 27


def test_v24_config_gains_silent_completion_sound(tmp_path, monkeypatch):
    import json

    from backend.app import config as config_module

    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 24,
                "token": "x" * 40,
                "download_dir": str(tmp_path / "downloads"),
                "ffmpeg_path": str(tmp_path / "ffmpeg.exe"),
                "download_speed_limit_kib": 128,
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setattr(config_module, "CONFIG_PATH", config_path)
    loaded = config_module.load_settings()
    assert loaded.config_version == 27
    assert loaded.completion_sound_enabled is False
    assert loaded.download_speed_limit_kib == 128
    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["config_version"] == 27
    assert saved["completion_sound_enabled"] is False
