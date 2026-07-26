import asyncio
import base64
import errno
import os
import time
from pathlib import Path

import httpx
import pytest

from backend.app.config import settings
from backend.app.downloader import http_file as http_file_module
from backend.app.downloader.http_file import HTTPDownloader, _content_disposition_filename, _SpeedWindow
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


def test_http_post_replay_uses_one_post_without_probe_or_ranges(tmp_path, monkeypatch):
    from backend.app.downloader import http_file as http_file_module

    payload = b'{"file":"report-2026"}'
    body = b"downloaded through post"
    requests = []
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    task = Task(
        id="post-replay",
        url="https://api.test/reports/export",
        task_type=TaskType.HTTP,
        request_method="POST",
        request_body=base64.b64encode(payload).decode("ascii"),
        request_headers={"content-type": "application/json"},
        concurrency=12,
    )

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append(request)
        assert request.method == "POST"
        assert request.content == payload
        assert "range" not in request.headers
        return httpx.Response(
            200,
            content=body,
            headers={
                "Content-Length": str(len(body)),
                "Content-Type": "application/pdf",
                "Content-Disposition": "attachment; filename=report.pdf",
            },
            request=request,
        )

    class MockClient(httpx.AsyncClient):
        def __init__(self, *args, **kwargs):
            kwargs["transport"] = httpx.MockTransport(handler)
            super().__init__(*args, **kwargs)

    monkeypatch.setattr(http_file_module.httpx, "AsyncClient", MockClient)
    asyncio.run(HTTPDownloader(task).run())

    assert [request.method for request in requests] == ["POST"]
    assert task.status.value == "done"
    assert Path(task.output_path).name == "report.pdf"
    assert Path(task.output_path).read_bytes() == body
    assert task.progress.max_workers == 1


def test_content_disposition_handles_rfc5987_and_quoted_semicolons():
    assert _content_disposition_filename("attachment; filename*=UTF-8''%E4%B8%8B%E8%BD%BD%3B%E6%B5%8B%E8%AF%95.iso") == "下载;测试.iso"
    assert _content_disposition_filename("attachment; filename*=ISO-8859-1''caf%E9.pdf") == "café.pdf"
    assert _content_disposition_filename('attachment; filename="archive; final.zip"') == "archive; final.zip"


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


def test_speed_window_measures_only_recent_transfer(monkeypatch):
    clock = {"now": 100.0}
    monkeypatch.setattr(http_file_module.time, "monotonic", lambda: clock["now"])
    window = _SpeedWindow(span_seconds=8.0)
    window.add(1024)
    clock["now"] = 101.0
    window.add(1024)
    assert window.speed() == pytest.approx(2048.0)
    # Samples older than the span age out entirely: after a stall the shown
    # speed drops to zero instead of clinging to a lifetime average.
    clock["now"] = 120.0
    assert window.speed() == 0.0


def test_resumed_bytes_do_not_inflate_speed_or_eta(monkeypatch):
    clock = {"now": 50.0}
    monkeypatch.setattr(http_file_module.time, "monotonic", lambda: clock["now"])
    task = Task(id="eta", url="https://files.test/f.bin", task_type=TaskType.HTTP)
    downloader = HTTPDownloader(task)
    window = _SpeedWindow()
    task.progress.total_bytes = 10 * 1024
    # Half the file was restored from a previous session; only 1 KiB has
    # actually been transferred in this one.
    task.progress.downloaded_bytes = 5 * 1024
    window.add(1024)
    clock["now"] = 51.0
    downloader._apply_speed(window)
    assert task.progress.speed_bytes_per_sec == pytest.approx(1024.0)
    assert task.progress.eta_seconds == pytest.approx(5.0)


def test_endgame_splits_tail_of_last_slow_chunk(tmp_path, monkeypatch):
    body = bytes(range(256)) * 32768  # 8 MiB
    monkeypatch.setattr(settings, "http_chunk_size_mb", 8)
    task = Task(
        id="endgame",
        url="https://files.test/big.bin",
        task_type=TaskType.HTTP,
        concurrency=2,
    )
    ranges: list[str] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        value = request.headers.get("range", "")
        ranges.append(value)
        start_text, end_text = value.removeprefix("bytes=").split("-", 1)
        start, end = int(start_text), int(end_text)
        if start == 0:
            # The primary connection is slow; idle workers must claim the
            # tail instead of waiting for it.
            await asyncio.sleep(0.25)
        return httpx.Response(
            206,
            content=body[start : end + 1],
            headers={"Content-Range": f"bytes {start}-{end}/{len(body)}"},
            request=request,
        )

    async def run():
        part = tmp_path / "payload.downloading"
        downloader = HTTPDownloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_ranges(
                client,
                {},
                part,
                tmp_path / "resume.json",
                {"total": len(body), "etag": '"v1"', "last_modified": "now"},
            )
        assert part.read_bytes() == body
        assert task.progress.completed_segments == 1
        assert task.progress.downloaded_bytes == len(body)
        # The idle worker split the in-flight chunk at least once.
        assert len(ranges) >= 2
        assert any(not value.startswith("bytes=0-") for value in ranges)

    asyncio.run(run())


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
    asyncio.run(run())
    assert part.read_bytes() == b"z" * 32
    assert task.progress.downloaded_bytes == 32


def test_http_range_429_holds_new_workers_until_shared_retry_after(tmp_path, monkeypatch):
    """A completed worker must not start its next range during a peer's 429 window."""
    from backend.app.downloader import http_file as http_file_module
    from backend.app.downloader.errors import SharedRetryWindow

    chunk_size = 1024 * 1024
    total = chunk_size * 3
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    state = {"limited": False, "deadline": 0.0, "requests": []}

    class RecordingRetryWindow(SharedRetryWindow):
        async def extend(self, delay: float):
            remaining, extended = await super().extend(delay)
            if extended:
                state["deadline"] = time.monotonic() + remaining
            return remaining, extended

    monkeypatch.setattr(http_file_module, "SharedRetryWindow", RecordingRetryWindow)
    task = Task(
        id="http-rate-limit",
        url="https://files.test/rate-limited.bin",
        task_type=TaskType.HTTP,
        concurrency=2,
    )

    async def handler(request: httpx.Request) -> httpx.Response:
        start_text, end_text = request.headers["range"].removeprefix("bytes=").split("-", 1)
        start, end = int(start_text), int(end_text)
        requested_at = time.monotonic()
        # Requests already in flight before the response is handled are valid;
        # every new request after the gate is armed must wait for its deadline.
        if state["deadline"]:
            assert requested_at >= state["deadline"] - 0.003
        state["requests"].append((start, requested_at))
        if start == 0 and not state["limited"]:
            state["limited"] = True
            return httpx.Response(
                429,
                headers={"Retry-After": "0.05"},
                request=request,
            )
        # Let the 429 worker arm the shared window before this worker takes
        # another item from the range queue.
        await asyncio.sleep(0.005)
        return httpx.Response(
            206,
            content=b"x" * (end - start + 1),
            headers={"Content-Range": f"bytes {start}-{end}/{total}"},
            request=request,
        )

    async def run() -> None:
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
    assert task.progress.completed_segments == 3
    assert len(state["requests"]) >= 4


def test_http_resume_without_a_server_validator_is_discarded(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    part = tmp_path / "payload.downloading"
    # A stale same-size source must not become an apparently complete result.
    part.write_bytes(b"old-old")
    state = tmp_path / "resume.json"
    state.write_text(
        '{"url":"https://files.test/a.bin","total":7,"etag":"","last_modified":"","completed":[0]}',
        encoding="utf-8",
    )
    task = Task(id="http-no-validator", url="https://files.test/a.bin", task_type=TaskType.HTTP, concurrency=1)

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["range"] == "bytes=0-6"
        return httpx.Response(
            206,
            content=b"current",
            headers={"Content-Range": "bytes 0-6/7"},
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
                {"total": 7, "etag": "", "last_modified": ""},
            )

    asyncio.run(run())
    assert part.read_bytes() == b"current"
    assert task.progress.downloaded_bytes == 7


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
