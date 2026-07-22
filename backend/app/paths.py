import os
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping


@dataclass(frozen=True)
class RuntimePaths:
    mode: str
    project_root: Path
    data_root: Path
    config_path: Path
    database_path: Path
    webview_path: Path
    default_download_dir: Path
    default_temp_dir: Path


def resolve_runtime_paths(
    *,
    frozen: bool | None = None,
    executable: Path | None = None,
    project_root: Path | None = None,
    environ: Mapping[str, str] | None = None,
) -> RuntimePaths:
    is_frozen = getattr(sys, "frozen", False) if frozen is None else frozen
    executable_path = Path(sys.executable if executable is None else executable).resolve()
    source_root = (
        Path(__file__).resolve().parent.parent.parent
        if project_root is None
        else Path(project_root).resolve()
    )
    environment = os.environ if environ is None else environ

    if not is_frozen:
        return RuntimePaths(
            mode="source",
            project_root=source_root,
            data_root=source_root,
            config_path=source_root / "config.json",
            database_path=source_root / "backend" / "data.db",
            webview_path=source_root / ".webview",
            default_download_dir=source_root / "downloads",
            default_temp_dir=source_root,
        )

    app_root = executable_path.parent
    if (app_root / "portable").is_file():
        return RuntimePaths(
            mode="portable",
            project_root=app_root,
            data_root=app_root,
            config_path=app_root / "config.json",
            database_path=app_root / "data.db",
            webview_path=app_root / ".webview",
            default_download_dir=app_root / "downloads",
            default_temp_dir=app_root,
        )

    local_app_data = environment.get("LOCALAPPDATA")
    user_profile = environment.get("USERPROFILE")
    data_root = Path(local_app_data) / "HLS Downloader" if local_app_data else app_root / ".data"
    downloads = (
        Path(user_profile) / "Downloads" / "HLS Downloader"
        if user_profile
        else app_root / "downloads"
    )
    return RuntimePaths(
        mode="installed",
        project_root=app_root,
        data_root=data_root,
        config_path=data_root / "config.json",
        database_path=data_root / "data.db",
        webview_path=data_root / "WebView",
        default_download_dir=downloads,
        default_temp_dir=app_root,
    )


def migrate_legacy_state(paths: RuntimePaths) -> list[Path]:
    if paths.mode != "installed":
        return []

    paths.data_root.mkdir(parents=True, exist_ok=True)
    migrated: list[Path] = []
    candidates = (
        (paths.project_root / "config.json", paths.config_path),
        (paths.project_root / "data.db", paths.database_path),
        (paths.project_root / "backend" / "data.db", paths.database_path),
    )
    for source, destination in candidates:
        if destination.exists() or not source.is_file():
            continue
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        migrated.append(destination)
    return migrated


RUNTIME_PATHS = resolve_runtime_paths()
migrate_legacy_state(RUNTIME_PATHS)
