import asyncio

from backend.app.downloader.task_manager import TaskManager
from backend.app.models import TaskType
from backend.app.torrent_watch import (
    TorrentWatchState,
    collect_new_torrents,
    read_watch_torrent,
)


async def _async_noop(*_args, **_kwargs):
    return None


def test_first_scan_does_not_import_existing_torrents(tmp_path):
    existing = tmp_path / "old.torrent"
    existing.write_bytes(b"d4:infod6:lengthi1e4:name3:abeee")
    state = TorrentWatchState()
    assert collect_new_torrents(str(tmp_path), state) == []
    assert collect_new_torrents(str(tmp_path), state) == []


def test_second_scan_imports_only_new_torrent(tmp_path):
    state = TorrentWatchState()
    (tmp_path / "old.torrent").write_bytes(b"old")
    assert collect_new_torrents(str(tmp_path), state) == []
    fresh = tmp_path / "new.torrent"
    fresh.write_bytes(b"new")
    found = collect_new_torrents(str(tmp_path), state)
    assert found == [fresh.resolve()]
    assert collect_new_torrents(str(tmp_path), state) == []


def test_disabled_or_missing_directory_clears_state(tmp_path):
    state = TorrentWatchState()
    (tmp_path / "a.torrent").write_bytes(b"a")
    collect_new_torrents(str(tmp_path), state)
    assert state.primed_dir
    assert collect_new_torrents("", state) == []
    assert state.primed_dir == ""
    assert collect_new_torrents(str(tmp_path / "missing"), state) == []


def test_read_watch_torrent_rejects_empty_and_non_torrent(tmp_path):
    other = tmp_path / "note.txt"
    other.write_text("nope", encoding="utf-8")
    try:
        read_watch_torrent(other)
        raise AssertionError("should reject")
    except ValueError:
        pass
    empty = tmp_path / "empty.torrent"
    empty.write_bytes(b"")
    try:
        read_watch_torrent(empty)
        raise AssertionError("should reject empty")
    except ValueError:
        pass
    ok = tmp_path / "ok.torrent"
    ok.write_bytes(b"abc")
    assert read_watch_torrent(ok) == b"abc"


def test_maintain_watch_imports_new_file_when_enabled(tmp_path, monkeypatch):
    from backend.app.downloader import task_manager as manager_module
    from backend.app import torrent_watch as watch_module

    monkeypatch.setattr(manager_module.settings, "watch_torrents", True)
    monkeypatch.setattr(manager_module.settings, "watch_dir", str(tmp_path))
    monkeypatch.setattr(manager_module, "legal_acceptance_current", lambda: True)
    watch_module.watch_state.disable()
    (tmp_path / "old.torrent").write_bytes(b"old")

    imported = []

    async def fake_import(self, content, *, name):
        imported.append((content, name))
        from backend.app.models import Task
        return Task(id="watched", url=f"torrent-file:{name}", task_type=TaskType.TORRENT)

    monkeypatch.setattr(TaskManager, "import_torrent_bytes", fake_import)

    async def run():
        manager = TaskManager()
        await manager._maintain_torrent_watch()
        assert imported == []
        (tmp_path / "fresh.torrent").write_bytes(b"fresh-bytes")
        await manager._maintain_torrent_watch()
        assert imported == [(b"fresh-bytes", "fresh.torrent")]
        await manager._maintain_torrent_watch()
        assert imported == [(b"fresh-bytes", "fresh.torrent")]

    asyncio.run(run())
