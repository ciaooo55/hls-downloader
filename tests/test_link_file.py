import asyncio
from pathlib import Path

import pytest
from pydantic import ValidationError

from backend.app.link_file import (
    LinkFileError,
    extract_download_url,
    read_link_file,
)
from backend.app.schemas import LinkPathImport, TaskCreate
from backend.app.torrent_watch import TorrentWatchState, collect_new_torrents


def test_parse_internet_shortcut_and_magnet_text():
    assert extract_download_url('[InternetShortcut]\nURL=https://cdn.example.test/a.mp4\n', suffix='.url') == 'https://cdn.example.test/a.mp4'
    assert extract_download_url('magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567', suffix='.magnet').startswith('magnet:')
    assert extract_download_url('ftp://nas.example.test/pub/file.bin').startswith('ftp://')


def test_reject_unsafe_or_empty_links():
    with pytest.raises(LinkFileError):
        extract_download_url('[InternetShortcut]\nURL=javascript:alert(1)\n', suffix='.url')
    with pytest.raises(LinkFileError):
        extract_download_url('[InternetShortcut]\nURL=file:///C:/Windows/notepad.exe\n', suffix='.url')
    with pytest.raises(LinkFileError):
        extract_download_url('magnet:?dn=nohash', suffix='.magnet')


def test_read_link_file_supports_utf16_url(tmp_path):
    path = tmp_path / 'movie.url'
    path.write_bytes('[InternetShortcut]\r\nURL=https://cdn.example.test/movie.mkv\r\n'.encode('utf-16'))
    assert read_link_file(path) == 'https://cdn.example.test/movie.mkv'


def test_watch_folder_picks_new_url_after_prime(tmp_path):
    state = TorrentWatchState()
    (tmp_path / 'old.url').write_text('[InternetShortcut]\nURL=https://cdn.example.test/old.mp4\n', encoding='utf-8')
    assert collect_new_torrents(str(tmp_path), state) == []
    fresh = tmp_path / 'new.magnet'
    fresh.write_text('magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567\n', encoding='utf-8')
    found = collect_new_torrents(str(tmp_path), state)
    assert found == [fresh.resolve()]


def test_link_path_schema_and_task_create_accept_extracted_url():
    body = LinkPathImport(path='C:/links/a.url')
    assert body.auto_start is True
    assert TaskCreate(url='https://cdn.example.test/a.mp4').url.startswith('https://')
    with pytest.raises(ValidationError):
        LinkPathImport(path='')

def test_maintain_watch_imports_new_url_file(tmp_path, monkeypatch):
    from backend.app.downloader import task_manager as manager_module
    from backend.app import torrent_watch as watch_module
    from backend.app.downloader.task_manager import TaskManager
    from backend.app.models import Task, TaskType

    monkeypatch.setattr(manager_module.settings, "watch_torrents", True)
    monkeypatch.setattr(manager_module.settings, "watch_dir", str(tmp_path))
    monkeypatch.setattr(manager_module, "legal_acceptance_current", lambda: True)
    watch_module.watch_state.disable()
    (tmp_path / "old.url").write_text("[InternetShortcut]\nURL=https://cdn.example.test/old.mp4\n", encoding="utf-8")
    imported = []

    async def fake_import(self, url, *, title="", auto_start=False):
        imported.append((url, title, auto_start))
        return Task(id="watched-url", url=url, task_type=TaskType.HTTP)

    monkeypatch.setattr(TaskManager, "import_link_url", fake_import)

    async def run():
        manager = TaskManager()
        await manager._maintain_torrent_watch()
        assert imported == []
        (tmp_path / "fresh.url").write_text("[InternetShortcut]\nURL=https://cdn.example.test/fresh.mp4\n", encoding="utf-8")
        await manager._maintain_torrent_watch()
        assert imported == [("https://cdn.example.test/fresh.mp4", "fresh", False)]

    asyncio.run(run())

from backend.app.link_file import extract_download_urls


def test_extracts_classic_m3u_file_list_and_rejects_local_hls_segments():
    songs = extract_download_urls(
        "#EXTM3U\n#EXTINF:123,A\nhttps://cdn.example.test/a.mp3\nhttps://cdn.example.test/b.mp3\n",
        suffix=".m3u",
    )
    assert songs == [
        "https://cdn.example.test/a.mp3",
        "https://cdn.example.test/b.mp3",
    ]
    master = extract_download_urls(
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\nhttps://cdn.example.test/1080.m3u8\n",
        suffix=".m3u8",
    )
    assert master == ["https://cdn.example.test/1080.m3u8"]
    with pytest.raises(LinkFileError, match="\u5206\u7247\u64ad\u653e\u5217\u8868"):
        extract_download_urls(
            "#EXTM3U\n#EXTINF:4,\nhttps://cdn.example.test/s0.ts\n#EXTINF:4,\nhttps://cdn.example.test/s1.ts\n#EXTINF:4,\nhttps://cdn.example.test/s2.ts\n",
            suffix=".m3u8",
        )


def test_html_import_keeps_absolute_files_and_drops_relative_page_links():
    urls = extract_download_urls(
        '<html><a href="https://cdn.example.test/setup.exe">app</a><a href="/local.mp4">nope</a></html>',
        suffix=".html",
    )
    assert urls == ["https://cdn.example.test/setup.exe"]


def test_watch_folder_still_ignores_playlists(tmp_path):
    state = TorrentWatchState()
    (tmp_path / "songs.m3u").write_text("https://cdn.example.test/a.mp3\n", encoding="utf-8")
    assert collect_new_torrents(str(tmp_path), state) == []

