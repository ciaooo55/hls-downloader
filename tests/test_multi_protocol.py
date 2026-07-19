import asyncio
from pathlib import Path

import httpx

from backend.app.config import settings
from backend.app.downloader.http_file import HTTPDownloader
from backend.app.downloader.torrent import TorrentDownloader
from backend.app.downloader.task_manager import resolve_task_type
from backend.app.models import Task, TaskType


def test_auto_task_type_recognizes_supported_sources():
    assert resolve_task_type(TaskType.AUTO, "https://cdn.test/video.m3u8?token=1") is TaskType.HLS
    assert resolve_task_type(TaskType.AUTO, "https://cdn.test/manifest.mpd") is TaskType.DASH
    assert resolve_task_type(TaskType.AUTO, "https://cdn.test/archive.zip") is TaskType.HTTP
    assert resolve_task_type(TaskType.AUTO, "magnet:?xt=urn:btih:abc") is TaskType.TORRENT
    assert resolve_task_type(TaskType.AUTO, "https://cdn.test/file.torrent") is TaskType.TORRENT


def test_http_range_downloader_writes_one_sparse_file_and_validates_ranges(tmp_path, monkeypatch):
    body = (b"0123456789abcdef" * 131072) + b"tail"
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    task = Task(
        id="http1",
        url="https://files.test/video.mp4",
        task_type=TaskType.HTTP,
        concurrency=3,
    )

    def handler(request: httpx.Request) -> httpx.Response:
        value = request.headers.get("range", "")
        start_text, end_text = value.removeprefix("bytes=").split("-", 1)
        start, end = int(start_text), int(end_text)
        return httpx.Response(
            206,
            content=body[start : end + 1],
            headers={"Content-Range": f"bytes {start}-{end}/{len(body)}"},
            request=request,
        )

    async def run():
        part = tmp_path / "payload.downloading"
        state = tmp_path / "resume.json"
        downloader = HTTPDownloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_ranges(
                client,
                {},
                part,
                state,
                {
                    "total": len(body),
                    "etag": '"v1"',
                    "last_modified": "now",
                },
            )
        assert part.read_bytes() == body
        assert task.progress.completed_segments == 3
        assert task.progress.progress_percent == 100

    asyncio.run(run())


def test_http_resume_is_discarded_when_etag_changes(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    part = tmp_path / "payload.downloading"
    part.write_bytes(b"x" * 32)
    state = tmp_path / "resume.json"
    state.write_text(
        '{"url":"https://files.test/a.bin","total":32,"etag":"old","last_modified":"","completed":[0]}',
        encoding="utf-8",
    )
    task = Task(
        id="http2",
        url="https://files.test/a.bin",
        task_type=TaskType.HTTP,
        concurrency=1,
    )

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            206,
            content=b"z" * 32,
            headers={"Content-Range": "bytes 0-31/32"},
            request=request,
        )

    async def run():
        downloader = HTTPDownloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_ranges(
                client,
                {},
                part,
                state,
                {"total": 32, "etag": "new", "last_modified": ""},
            )
        assert part.read_bytes() == b"z" * 32
        assert task.progress.downloaded_bytes == 32

    asyncio.run(run())


def test_torrent_downloads_from_local_peer_and_stops_at_completion(tmp_path, monkeypatch):
    import libtorrent as lt

    seed_root = tmp_path / "seed"
    seed_root.mkdir()
    content = b"local torrent payload" * 32768
    (seed_root / "sample.bin").write_bytes(content)
    storage = lt.file_storage()
    lt.add_files(storage, str(seed_root / "sample.bin"))
    creator = lt.create_torrent(storage, 16384)
    lt.set_piece_hashes(creator, str(seed_root))
    torrent_path = tmp_path / "sample.torrent"
    torrent_path.write_bytes(lt.bencode(creator.generate()))
    info = lt.torrent_info(str(torrent_path))
    seed_session = lt.session(
        {
            "listen_interfaces": "127.0.0.1:0",
            "enable_dht": False,
            "enable_lsd": False,
            "enable_upnp": False,
            "enable_natpmp": False,
        }
    )
    seed_session.add_torrent(
        {
            "ti": info,
            "save_path": str(seed_root),
            "flags": lt.torrent_flags.seed_mode,
        }
    )
    download_root = tmp_path / "downloads"
    download_root.mkdir()
    monkeypatch.setattr(settings, "download_dir", str(download_root))
    monkeypatch.setattr(settings, "bt_enable_dht", False)
    task = Task(
        id="bt1",
        url="torrent-file:sample.torrent",
        task_type=TaskType.TORRENT,
        engine_state={
            "torrent_path": str(torrent_path),
            "peers": [f"127.0.0.1:{seed_session.listen_port()}"],
        },
    )
    task.cancel_event = asyncio.Event()
    task.pause_event = asyncio.Event()

    asyncio.run(TorrentDownloader(task).run())

    assert task.status.value == "done"
    output = Path(task.output_path)
    assert output.read_bytes() == content
    assert task.progress.progress_percent == 100
