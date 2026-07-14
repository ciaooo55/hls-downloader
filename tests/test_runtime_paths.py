import json
from pathlib import Path

from backend.app.paths import migrate_legacy_state, resolve_runtime_paths


def test_source_runtime_keeps_project_relative_state(tmp_path):
    paths = resolve_runtime_paths(
        frozen=False,
        executable=tmp_path / "python.exe",
        project_root=tmp_path / "source",
        environ={},
    )

    assert paths.mode == "source"
    assert paths.data_root == tmp_path / "source"
    assert paths.config_path == tmp_path / "source" / "config.json"
    assert paths.database_path == tmp_path / "source" / "backend" / "data.db"
    assert paths.default_download_dir == tmp_path / "source" / "downloads"


def test_portable_runtime_uses_executable_directory(tmp_path):
    app_dir = tmp_path / "portable-app"
    app_dir.mkdir()
    (app_dir / "portable").write_text("", encoding="ascii")

    paths = resolve_runtime_paths(
        frozen=True,
        executable=app_dir / "HLSDownloader.exe",
        project_root=tmp_path / "source",
        environ={"LOCALAPPDATA": str(tmp_path / "local")},
    )

    assert paths.mode == "portable"
    assert paths.project_root == app_dir
    assert paths.data_root == app_dir
    assert paths.database_path == app_dir / "data.db"
    assert paths.webview_path == app_dir / ".webview"


def test_installed_runtime_separates_program_data_and_downloads(tmp_path):
    app_dir = tmp_path / "Programs" / "HLS Downloader"
    local = tmp_path / "LocalAppData"
    profile = tmp_path / "User"

    paths = resolve_runtime_paths(
        frozen=True,
        executable=app_dir / "HLSDownloader.exe",
        project_root=tmp_path / "source",
        environ={"LOCALAPPDATA": str(local), "USERPROFILE": str(profile)},
    )

    assert paths.mode == "installed"
    assert paths.project_root == app_dir
    assert paths.data_root == local / "HLS Downloader"
    assert paths.config_path == local / "HLS Downloader" / "config.json"
    assert paths.database_path == local / "HLS Downloader" / "data.db"
    assert paths.webview_path == local / "HLS Downloader" / "WebView"
    assert paths.default_download_dir == profile / "Downloads" / "HLS Downloader"


def test_installed_runtime_falls_back_when_windows_environment_is_missing(tmp_path):
    app_dir = tmp_path / "app"

    paths = resolve_runtime_paths(
        frozen=True,
        executable=app_dir / "HLSDownloader.exe",
        project_root=tmp_path / "source",
        environ={},
    )

    assert paths.data_root == app_dir / ".data"
    assert paths.default_download_dir == app_dir / "downloads"


def test_legacy_state_is_copied_once_without_overwriting_new_state(tmp_path):
    app_dir = tmp_path / "Programs" / "HLS Downloader"
    app_dir.mkdir(parents=True)
    local = tmp_path / "LocalAppData"
    legacy_config = {"token": "legacy", "download_dir": "downloads"}
    (app_dir / "config.json").write_text(json.dumps(legacy_config), encoding="utf-8")
    (app_dir / "data.db").write_bytes(b"legacy-db")
    paths = resolve_runtime_paths(
        frozen=True,
        executable=app_dir / "HLSDownloader.exe",
        project_root=tmp_path / "source",
        environ={"LOCALAPPDATA": str(local), "USERPROFILE": str(tmp_path / "User")},
    )

    migrated = migrate_legacy_state(paths)

    assert migrated == [paths.config_path, paths.database_path]
    assert json.loads(paths.config_path.read_text(encoding="utf-8")) == legacy_config
    assert paths.database_path.read_bytes() == b"legacy-db"

    paths.config_path.write_text('{"token":"new"}', encoding="utf-8")
    assert migrate_legacy_state(paths) == []
    assert paths.config_path.read_text(encoding="utf-8") == '{"token":"new"}'


def test_legacy_backend_database_is_recognized(tmp_path):
    app_dir = tmp_path / "app"
    (app_dir / "backend").mkdir(parents=True)
    (app_dir / "backend" / "data.db").write_bytes(b"source-layout")
    paths = resolve_runtime_paths(
        frozen=True,
        executable=app_dir / "HLSDownloader.exe",
        project_root=tmp_path / "source",
        environ={"LOCALAPPDATA": str(tmp_path / "local")},
    )

    migrate_legacy_state(paths)

    assert paths.database_path.read_bytes() == b"source-layout"
