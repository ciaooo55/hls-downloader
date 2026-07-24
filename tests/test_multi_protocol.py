import asyncio
import errno
import os
from pathlib import Path

import httpx

from backend.app.config import settings
from backend.app.downloader.http_file import HTTPDownloader
from backend.app.downloader.engine import publish_path, task_work_dir
from backend.app.downloader.torrent import TorrentDownloader
from backend.app.downloader import task_manager as task_manager_module
from backend.app.downloader.task_manager import TaskManager, resolve_task_type
from backend.app.models import Task, TaskType


def test_auto_task_type_recognizes_supported_sources():
    assert resolve_task_type(TaskType.AUTO, "https://cdn.test/video.m3u8?token=1") is TaskType.HLS
    assert resolve_task_type(TaskType.AUTO, "https://cdn.test/manifest.mpd") is TaskType.DASH
    assert resolve_task_type(TaskType.AUTO, "https://cdn.test/archive.zip") is TaskType.HTTP
    assert resolve_task_type(TaskType.AUTO, "magnet:?xt=urn:btih:abc") is TaskType.TORRENT
    assert resolve_task_type(TaskType.AUTO, "https://cdn.test/file.torrent") is TaskType.TORRENT
    assert resolve_task_type(TaskType.AUTO, "https://cdn.test/stream?id=1", "application/vnd.apple.mpegurl") is TaskType.HLS
    assert resolve_task_type(TaskType.AUTO, "https://cdn.test/manifest?id=1", "application/dash+xml; charset=utf-8") is TaskType.DASH


def test_create_task_uses_captured_manifest_mime_when_url_has_no_extension(monkeypatch):
    async def no_db(*args, **kwargs):
        return None

    async def run():
        manager = TaskManager()
        monkeypatch.setattr(task_manager_module, "run_db", no_db)
        hls = await manager.create_task(
            "https://cdn.test/play?id=one",
            mime_type="application/vnd.apple.mpegurl; charset=utf-8",
        )
        dash = await manager.create_task(
            "https://cdn.test/manifest?id=two",
            mime_type="application/dash+xml",
        )
        assert hls.task_type is TaskType.HLS
        assert dash.task_type is TaskType.DASH

    asyncio.run(run())


def test_http_probe_verifies_range_when_head_omits_accept_ranges():
    task = Task(id="probe-range", url="http://files.test/100MB.zip", task_type=TaskType.HTTP)

    def handler(request: httpx.Request) -> httpx.Response:
        if request.method == "HEAD":
            return httpx.Response(200, headers={"Content-Length": "104857600", "Content-Type": "application/zip"}, request=request)
        assert request.headers["range"] == "bytes=0-0"
        return httpx.Response(206, content=b"x", headers={"Content-Range": "bytes 0-0/104857600"}, request=request)

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler), follow_redirects=True) as client:
            return await HTTPDownloader(task)._probe(client, {})

    metadata = asyncio.run(run())
    assert metadata["ranges"] is True
    assert metadata["total"] == 104857600


def test_http_probe_follows_https_to_http_redirect_and_uses_server_filename():
    task = Task(id="probe-redirect", url="https://mirror.test/download?id=1", task_type=TaskType.HTTP)

    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.host == "mirror.test":
            return httpx.Response(302, headers={"Location": "http://cdn.test/releases/system.iso"}, request=request)
        if request.method == "HEAD":
            return httpx.Response(405, request=request)
        return httpx.Response(206, content=b"x", headers={
            "Content-Range": "bytes 0-0/5500000000",
            "Content-Disposition": "attachment; filename=ubuntu-desktop.iso",
            "Content-Type": "application/octet-stream",
        }, request=request)

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler), follow_redirects=True) as client:
            return await HTTPDownloader(task)._probe(client, {})

    metadata = asyncio.run(run())
    assert metadata["ranges"] is True
    assert metadata["total"] == 5500000000
    assert metadata["filename"] == "ubuntu-desktop.iso"
    assert metadata["final_url"] == "http://cdn.test/releases/system.iso"


def test_task_process_files_use_configured_temp_directory(tmp_path):
    task = Task(
        id="temp-location",
        url="https://files.test/archive.zip",
        task_type=TaskType.HTTP,
        engine_state={"temp_dir": str(tmp_path / "process")},
    )

    assert task_work_dir(task) == tmp_path / "process" / ".tasks" / task.id


def test_publish_path_falls_back_to_copy_for_cross_drive_errors(tmp_path, monkeypatch):
    source = tmp_path / "cache" / "payload.downloading"
    destination = tmp_path / "output" / "archive.zip"
    source.parent.mkdir()
    destination.parent.mkdir()
    source.write_bytes(b"downloaded payload")
    destination.write_bytes(b"")
    real_replace = os.replace
    attempts = 0

    def cross_drive_once(src, dst):
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            error = OSError(errno.EACCES, "different drive")
            error.winerror = 17
            raise error
        return real_replace(src, dst)

    monkeypatch.setattr(os, "replace", cross_drive_once)

    publish_path(source, destination)

    assert destination.read_bytes() == b"downloaded payload"
    assert not source.exists()
    assert attempts == 2


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


def test_http_range_downloader_uses_twelve_workers_by_default(tmp_path, monkeypatch):
    chunk_size = 1024 * 1024
    total = chunk_size * 13
    active = 0
    peak = 0
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    task = Task(
        id="http12",
        url="https://files.test/archive.bin",
        task_type=TaskType.HTTP,
        concurrency=12,
    )

    async def handler(request: httpx.Request) -> httpx.Response:
        nonlocal active, peak
        start_text, end_text = request.headers["range"].removeprefix("bytes=").split("-", 1)
        start, end = int(start_text), int(end_text)
        active += 1
        peak = max(peak, active)
        await asyncio.sleep(0.02)
        active -= 1
        return httpx.Response(
            206,
            content=b"x" * (end - start + 1),
            headers={"Content-Range": f"bytes {start}-{end}/{total}"},
            request=request,
        )

    async def run():
        downloader = HTTPDownloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_ranges(
                client,
                {},
                tmp_path / "payload.downloading",
                tmp_path / "resume.json",
                {"total": total, "etag": '"v1"', "last_modified": "now"},
            )

    asyncio.run(run())
    assert task.progress.max_workers == 12
    assert peak == 12


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


def test_torrent_waits_for_disk_cache_before_finalizing():
    class Handle:
        def __init__(self):
            self.flush_calls = 0

        def flush_cache(self):
            self.flush_calls += 1

    handle = Handle()

    class CacheFlushedAlert:
        def __init__(self):
            self.handle = handle

    class Libtorrent:
        cache_flushed_alert = CacheFlushedAlert

    class Session:
        def __init__(self):
            self.polls = 0

        def pop_alerts(self):
            self.polls += 1
            return [] if self.polls == 1 else [CacheFlushedAlert()]

    task = Task(id="bt-flush", url="magnet:?xt=urn:btih:test", task_type=TaskType.TORRENT)
    downloader = TorrentDownloader(task)
    session = Session()

    asyncio.run(downloader._flush_storage(Libtorrent, session, handle, timeout=1))

    assert handle.flush_calls == 1
    assert session.polls == 2
