import asyncio

from backend.app import download_category as category_module
from backend.app.download_category import download_category, resolve_category_output_dir
from backend.app.downloader.task_manager import TaskManager
from backend.app.models import TaskType


async def _async_noop(*_args, **_kwargs):
    return None


def test_download_category_matches_desktop_groups():
    assert download_category("movie.mp4") == "media"
    assert download_category("cover.webp") == "media"
    assert download_category("setup.exe") == "program"
    assert download_category("files.7z") == "archive"
    assert download_category("manual.pdf") == "other"
    assert download_category("download.php", "text/plain", "hls") == "media"
    assert download_category("manifest", "", "dash") == "media"
    assert download_category("blob", "video/mp4") == "media"


def test_explicit_custom_directory_is_never_reclassified(tmp_path, monkeypatch):
    monkeypatch.setattr(category_module.settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(category_module.settings, "auto_category_dirs", True)
    monkeypatch.setattr(category_module.settings, "browser_category_dirs", {"media": str(tmp_path / "videos")})
    custom = tmp_path / "picked"
    custom.mkdir()
    assert resolve_category_output_dir(filename="a.mp4", explicit_dir=str(custom)) == str(custom.resolve())


def test_default_download_dir_still_allows_auto_classification(tmp_path, monkeypatch):
    root_dir = tmp_path / "downloads"
    root_dir.mkdir()
    monkeypatch.setattr(category_module.settings, "download_dir", str(root_dir))
    monkeypatch.setattr(category_module.settings, "auto_category_dirs", True)
    monkeypatch.setattr(category_module.settings, "browser_category_dirs", {})
    assert resolve_category_output_dir(
        filename="film.mkv",
        explicit_dir=str(root_dir),
    ) == str((root_dir / "媒体").resolve())
    monkeypatch.setattr(category_module.settings, "auto_category_dirs", False)
    assert resolve_category_output_dir(filename="film.mkv", explicit_dir=str(root_dir)) == ""


def test_configured_category_dir_wins_over_auto_subdir(tmp_path, monkeypatch):
    videos = tmp_path / "Videos"
    videos.mkdir()
    monkeypatch.setattr(category_module.settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(category_module.settings, "auto_category_dirs", True)
    monkeypatch.setattr(category_module.settings, "browser_category_dirs", {"media": str(videos)})
    assert resolve_category_output_dir(filename="song.mp3") == str(videos.resolve())
    assert resolve_category_output_dir(filename="app.exe") == str((tmp_path / "downloads" / "程序").resolve())


def test_create_task_stores_category_dir_only_when_policy_applies(tmp_path, monkeypatch):
    downloads = tmp_path / "downloads"
    downloads.mkdir()
    monkeypatch.setattr("backend.app.downloader.task_manager.settings.download_dir", str(downloads))
    monkeypatch.setattr(category_module.settings, "download_dir", str(downloads))
    monkeypatch.setattr(category_module.settings, "auto_category_dirs", False)
    monkeypatch.setattr(category_module.settings, "browser_category_dirs", {})

    async def run():
        manager = TaskManager()
        monkeypatch.setattr(manager, "_save_db", _async_noop)
        monkeypatch.setattr("backend.app.downloader.task_manager.run_db", _async_noop)
        plain = await manager.create_task("https://cdn.example.test/film.mp4", task_type=TaskType.HTTP, filename="film.mp4")
        assert "output_dir" not in plain.engine_state
        monkeypatch.setattr(category_module.settings, "auto_category_dirs", True)
        classified = await manager.create_task("https://cdn.example.test/film.mp4", task_type=TaskType.HTTP, filename="film.mp4")
        assert classified.engine_state["output_dir"] == str((downloads / "媒体").resolve())
        custom = tmp_path / "keep-here"
        custom.mkdir()
        kept = await manager.create_task(
            "https://cdn.example.test/film.mp4",
            task_type=TaskType.HTTP,
            filename="film.mp4",
            output_dir=str(custom),
        )
        assert kept.engine_state["output_dir"] == str(custom.resolve())

    asyncio.run(run())
