import importlib
import sys
from pathlib import Path


def test_project_root_uses_executable_directory_when_frozen(monkeypatch, tmp_path):
    exe = tmp_path / "install" / "HLSDownloader.exe"
    exe.parent.mkdir()
    exe.write_bytes(b"")

    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(exe))

    import backend.app.config as config

    reloaded = importlib.reload(config)
    try:
        assert reloaded.PROJECT_ROOT == exe.parent
        assert reloaded.CONFIG_PATH == exe.parent / "config.json"
        assert reloaded.settings.download_dir == str((exe.parent / "downloads").resolve())
        assert reloaded.settings.ffmpeg_path == str((exe.parent / "bin" / "ffmpeg.exe").resolve())
    finally:
        monkeypatch.delattr(sys, "frozen", raising=False)
        importlib.reload(config)


def test_ui_dist_uses_project_root(monkeypatch, tmp_path):
    exe = tmp_path / "install" / "HLSDownloader.exe"
    exe.parent.mkdir()
    exe.write_bytes(b"")

    monkeypatch.setattr(sys, "frozen", True, raising=False)
    monkeypatch.setattr(sys, "executable", str(exe))

    import backend.app.config as config
    import backend.app.main as main

    importlib.reload(config)
    reloaded_main = importlib.reload(main)
    try:
        assert reloaded_main.UI_DIST == exe.parent / "frontend" / "dist"
    finally:
        monkeypatch.delattr(sys, "frozen", raising=False)
        importlib.reload(config)
        importlib.reload(main)


def test_installer_build_and_nsis_include_userscript():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")
    nsis_script = (root / "installer" / "hls-downloader.nsi").read_text(encoding="utf-8")

    assert 'Join-Path $Root "userscript"' in build_script
    assert 'Join-Path $StageDir "userscript"' in build_script
    assert '${STAGE_DIR}\\userscript\\m3u8-sniffer.user.js' in nsis_script
    assert 'RMDir /r "$INSTDIR\\userscript"' in nsis_script
