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


def test_windows_build_emits_setup_and_portable_assets():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")

    assert "HLSDownloader-Windows-x64-Setup.exe" in build_script
    assert "HLSDownloader-Windows-x64-Portable.zip" in build_script
    assert 'Join-Path $PortableStage "portable"' in build_script
    assert "Compress-Archive" in build_script


def test_windows_build_uses_tools_from_path_on_clean_runner():
    root = Path(__file__).resolve().parent.parent
    build_script = (root / "scripts" / "build_installer.ps1").read_text(encoding="utf-8")

    assert 'Get-Command "makensis.exe"' in build_script
    assert 'Copy-MediaTool "ffmpeg.exe"' in build_script
    assert 'Copy-MediaTool "ffprobe.exe"' in build_script
    assert "Get-Command $Name" in build_script
    assert "Copy-Item" in build_script
    assert '$StageDir, $ReleaseDir, $BinDir, $ToolsDir' in build_script
    assert "return $installedCandidates[0]" not in build_script


def test_source_only_gitignore_excludes_generated_binaries():
    root = Path(__file__).resolve().parent.parent
    ignore = (root / ".gitignore").read_text(encoding="utf-8")

    assert "bin/" in ignore
    assert "release/" in ignore
    assert "backend/dist/" in ignore
    assert "frontend/dist/" in ignore
