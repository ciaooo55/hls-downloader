from backend.app.config import Settings


def test_download_overlay_windows_default_on():
    loaded = Settings()
    assert loaded.download_progress_window_enabled is True
    assert loaded.download_complete_popup_enabled is True
    assert loaded.config_version == 28


def test_v27_config_gains_idm_style_windows(tmp_path, monkeypatch):
    import json

    from backend.app import config as config_module

    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "config_version": 27,
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
    assert loaded.download_progress_window_enabled is True
    assert loaded.download_complete_popup_enabled is True
    assert loaded.completion_sound_enabled is True
    saved = json.loads(config_path.read_text(encoding="utf-8"))
    assert saved["config_version"] == 28
    assert saved["download_progress_window_enabled"] is True
    assert saved["download_complete_popup_enabled"] is True
