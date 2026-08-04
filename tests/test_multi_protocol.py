import asyncio
import base64
import errno
import json
import os
import time
from pathlib import Path

import httpx
import pytest

from backend.app.config import settings
from backend.app.downloader import http_file as http_file_module
from backend.app.downloader.http_file import (
    HTTPDownloader,
    _content_disposition_filename,
    _ensure_filename_extension,
    _parse_content_range,
    _SpeedWindow,
)
from backend.app.downloader.errors import DownloadError
from backend.app.downloader.engine import publish_path, task_work_dir
from backend.app.downloader.torrent import TorrentDownloader
from backend.app.downloader import task_manager as task_manager_module
from backend.app.downloader.task_manager import TaskManager, resolve_task_type
from backend.app.models import Task, TaskStatus, TaskType


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


def test_http_probe_verifies_range_without_using_head():
    task = Task(id="probe-range", url="http://files.test/100MB.zip", task_type=TaskType.HTTP)

    def handler(request: httpx.Request) -> httpx.Response:
        if request.method == "HEAD":
            return httpx.Response(200, headers={"Content-Length": "104857600", "Content-Type": "application/zip"}, request=request)
        assert request.headers["range"] == "bytes=0-255"
        return httpx.Response(
            206,
            content=b"x" * 256,
            headers={"Content-Range": "bytes 0-255/104857600"},
            request=request,
        )

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler), follow_redirects=True) as client:
            return await HTTPDownloader(task)._probe(client, {})

    metadata = asyncio.run(run())
    assert metadata["ranges"] is True
    assert metadata["total"] == 104857600


def test_http_probe_rejects_successful_html_error_page():
    task = Task(id="probe-html", url="https://files.test/archive.zip", task_type=TaskType.HTTP)
    body = b"<!doctype html><html><title>Sign in</title></html>"

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            206,
            content=body,
            headers={
                "Content-Range": f"bytes 0-{len(body) - 1}/{len(body)}",
                "Content-Type": "text/html",
            },
            request=request,
        )

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await HTTPDownloader(task)._probe(client, {})

    with pytest.raises(DownloadError) as raised:
        asyncio.run(run())
    assert raised.value.details.code == "HTTP_UNEXPECTED_CONTENT"


def test_http_probe_falls_back_to_plain_streamed_get_when_range_is_rejected():
    task = Task(id="probe-no-range", url="https://files.test/video.mp4", task_type=TaskType.HTTP)
    requests = []

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append((request.method, request.headers.get("range", "")))
        if request.headers.get("range"):
            return httpx.Response(416, request=request)
        return httpx.Response(
            200,
            headers={"Content-Length": "1234", "Content-Type": "video/mp4"},
            request=request,
        )

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            return await HTTPDownloader(task)._probe(client, {})

    metadata = asyncio.run(run())
    assert requests == [("GET", "bytes=0-255"), ("GET", "")]
    assert metadata["ranges"] is False
    assert metadata["total"] == 1234


def test_http_probe_does_not_use_short_content_length_when_range_total_is_unknown():
    task = Task(id="probe-unknown-range", url="https://files.test/video.mp4", task_type=TaskType.HTTP)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            206,
            content=b"x" * 256,
            headers={"Content-Range": "bytes 0-255/*", "Content-Length": "256", "Content-Type": "video/mp4"},
            request=request,
        )

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            return await HTTPDownloader(task)._probe(client, {})

    metadata = asyncio.run(run())
    assert metadata["ranges"] is False
    assert metadata["total"] == 0


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


def test_http_probe_reports_get_rejection_without_waiting_for_head():
    task = Task(id="probe-forbidden", url="https://files.test/signed.mp4", task_type=TaskType.HTTP)
    methods = []

    def handler(request: httpx.Request) -> httpx.Response:
        methods.append(request.method)
        return httpx.Response(403, request=request)

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await HTTPDownloader(task)._probe(client, {})

    with pytest.raises(httpx.HTTPStatusError):
        asyncio.run(run())
    assert methods == ["GET"]


def test_http_probe_retries_transient_server_failure_before_succeeding(monkeypatch):
    task = Task(id="probe-retry", url="https://files.test/video.mp4", task_type=TaskType.HTTP)
    attempts = 0

    def handler(request: httpx.Request) -> httpx.Response:
        nonlocal attempts
        attempts += 1
        if attempts < 3:
            return httpx.Response(503, request=request)
        return httpx.Response(
            206,
            headers={"Content-Range": "bytes 0-255/4096", "Content-Type": "video/mp4"},
            request=request,
        )

    async def run():
        downloader = HTTPDownloader(task)
        monkeypatch.setattr(http_file_module, "retry_delay_seconds", lambda *args, **kwargs: 0)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            return await downloader._probe_with_retry(client, downloader._headers())

    metadata = asyncio.run(run())
    assert attempts == 3
    assert metadata["total"] == 4096
    assert task.progress.reconnect_count == 2


def test_http_run_stops_with_actionable_error_when_metadata_probe_never_returns(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    monkeypatch.setattr(http_file_module, "PROBE_TOTAL_TIMEOUT", 0.01)
    task = Task(id="probe-timeout", url="https://files.test/slow.mp4", task_type=TaskType.HTTP)

    async def never_finishes(self, client, headers):
        await asyncio.sleep(1)

    monkeypatch.setattr(HTTPDownloader, "_probe", never_finishes)
    asyncio.run(HTTPDownloader(task).run())

    assert task.status.value == "failed"
    assert task.error_code == "HTTP_PROBE_TIMEOUT"
    assert task.stage == "failed"
    assert "准备下载" in task.error_hint


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


def test_http_content_range_and_mime_filename_cover_non_media_files():
    assert _parse_content_range("bytes 4-9/10") == (4, 9, 10)
    assert _parse_content_range("4-9/10") == (4, 9, 10)
    assert _parse_content_range("bytes 4-9/*") == (4, 9, None)
    assert _parse_content_range("bytes 9-4/10") is None
    assert _ensure_filename_extension("download", "application/pdf") == "download.pdf"
    assert _ensure_filename_extension("release", "application/x-7z-compressed") == "release.7z"
    assert _ensure_filename_extension("unknown", "application/octet-stream") == "unknown"
    assert _ensure_filename_extension("already.tar.gz", "application/gzip") == "already.tar.gz"


def test_http_probe_requests_identity_and_rejects_compressed_range_metadata():
    task = Task(id="probe-encoding", url="https://files.test/download", task_type=TaskType.HTTP)

    def handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["accept-encoding"] == "identity"
        return httpx.Response(
            206,
            # The probe never reads the body.  A raw empty stream keeps httpx
            # from trying to decompress the deliberately header-only example.
            stream=httpx.ByteStream(b""),
            headers={
                "Content-Range": "0-19/200",
                "Content-Length": "20",
                "Content-Encoding": "gzip",
                "Content-Type": "application/pdf",
            },
            request=request,
        )

    async def run():
        downloader = HTTPDownloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            return await downloader._probe(client, downloader._headers())

    metadata = asyncio.run(run())
    assert metadata["ranges"] is False
    assert metadata["total"] == 0
    assert metadata["content_type"] == "application/pdf"


def test_http_probe_does_not_wait_forever_for_first_chunk_when_headers_are_reliable(monkeypatch):
    task = Task(id="probe-header-first", url="https://files.test/video.mp4", task_type=TaskType.HTTP)
    monkeypatch.setattr(http_file_module, "PROBE_RESPONSE_TIMEOUT", 0.01)

    class DelayedBody(httpx.AsyncByteStream):
        async def __aiter__(self):
            await asyncio.sleep(1)
            yield b"not-read"

        async def aclose(self):
            return None

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            206,
            stream=DelayedBody(),
            headers={
                "Content-Range": "bytes 0-255/104857600",
                "Content-Length": "256",
                "Content-Type": "video/mp4",
            },
            request=request,
        )

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            return await HTTPDownloader(task)._probe(client, {})

    metadata = asyncio.run(run())
    assert metadata["ranges"] is True
    assert metadata["total"] == 104857600


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


def test_http_range_downloader_never_creates_zero_worker_pool(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    task = Task(
        id="legacy-zero-workers",
        url="https://files.test/archive.bin",
        task_type=TaskType.HTTP,
        concurrency=0,
    )
    body = b"legacy-data"

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            206,
            content=body,
            headers={"Content-Range": f"bytes 0-{len(body) - 1}/{len(body)}"},
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
                {"total": len(body), "etag": '"v1"', "last_modified": ""},
            )

    asyncio.run(run())
    assert task.progress.max_workers == 1
    assert task.progress.completed_segments == 1


def test_http_playback_fetches_requested_tail_while_normal_worker_is_blocked(tmp_path, monkeypatch):
    chunk_size = 1024 * 1024
    body = (b"a" * chunk_size) + (b"b" * chunk_size)
    tail_start = len(body) - 64
    normal_started = asyncio.Event()
    release_normal = asyncio.Event()
    requested: list[str] = []
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    task = Task(
        id="http-playback-priority",
        url="https://files.test/video.mp4",
        task_type=TaskType.HTTP,
        concurrency=1,
    )

    async def handler(request: httpx.Request) -> httpx.Response:
        value = request.headers["range"]
        requested.append(value)
        start_text, end_text = value.removeprefix("bytes=").split("-", 1)
        start, end = int(start_text), int(end_text)
        if start != tail_start:
            normal_started.set()
            await release_normal.wait()
        return httpx.Response(
            206,
            content=body[start : end + 1],
            headers={"Content-Range": f"bytes {start}-{end}/{len(body)}"},
            request=request,
        )

    async def run():
        part = tmp_path / "payload.downloading"
        downloader = HTTPDownloader(task)
        downloader._part_path = part
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            download = asyncio.create_task(downloader._download_ranges(
                client,
                {},
                part,
                tmp_path / "resume.json",
                {"total": len(body), "etag": '"v1"', "last_modified": "now"},
            ))
            await normal_started.wait()
            ready = await downloader.wait_for_range(tail_start, len(body) - 1, timeout=1)
            assert ready == part
            with part.open("rb") as stream:
                stream.seek(tail_start)
                assert stream.read(64) == b"b" * 64
            assert f"bytes={tail_start}-{len(body) - 1}" in requested
            release_normal.set()
            await download

    asyncio.run(run())
    assert task.progress.max_workers == 1
    assert task.progress.completed_segments == 2


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


def test_http_headers_force_identity_even_when_browser_captured_compression():
    task = Task(
        id="http-identity",
        url="https://files.test/archive.bin",
        task_type=TaskType.HTTP,
        request_headers={"Accept-Encoding": "gzip, br"},
    )

    assert HTTPDownloader(task)._headers()["Accept-Encoding"] == "identity"


def test_http_resume_rejects_weak_etag_without_last_modified(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    body = b"current"
    part = tmp_path / "payload.downloading"
    part.write_bytes(b"old-old")
    state = tmp_path / "resume.json"
    state.write_text(
        '{"version":2,"url":"https://files.test/a.bin","total":7,'
        '"etag":"W/\\"same\\"","last_modified":"","ranges":['
        '{"index":0,"from":0,"to":6,"current":7}]}',
        encoding="utf-8",
    )
    task = Task(id="http-weak", url="https://files.test/a.bin", task_type=TaskType.HTTP, concurrency=1)
    requests = []

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append(request.headers["range"])
        return httpx.Response(
            206,
            content=body,
            headers={"Content-Range": "bytes 0-6/7", "ETag": 'W/"same"'},
            request=request,
        )

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await HTTPDownloader(task)._download_ranges(
                client,
                {},
                part,
                state,
                {"total": 7, "etag": 'W/"same"', "last_modified": ""},
            )

    asyncio.run(run())
    assert requests == ["bytes=0-6"]
    assert part.read_bytes() == body


def test_http_resume_url_identity_keeps_new_signature_but_rejects_other_resource():
    from backend.app.downloader.http_file import _resume_resource_identity

    old = "https://cdn.test/file.mp4?quality=1080&s=old&e=100&_t=90"
    refreshed = "https://cdn.test/file.mp4?_t=190&e=200&s=new&quality=1080"
    other = "https://cdn.test/file.mp4?quality=720&s=new&e=200&_t=190"

    assert _resume_resource_identity(old) == _resume_resource_identity(refreshed)
    assert _resume_resource_identity(old) != _resume_resource_identity(other)


def test_http_v2_resume_keeps_partial_chunk_across_signed_url_change(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    body = b"0123456789"
    part = tmp_path / "payload.downloading"
    part.write_bytes(body[:4] + b"\0" * 6)
    state = tmp_path / "resume.json"
    state.write_text(json.dumps({
        "version": 2,
        "url": "https://cdn.test/file.mp4?token=old",
        "total": len(body),
        "etag": '\"same-file\"',
        "last_modified": "",
        "ranges": [{"index": 0, "from": 0, "to": 9, "current": 4}],
    }), encoding="utf-8")
    task = Task(id="signed", url="https://cdn.test/file.mp4?token=new", task_type=TaskType.HTTP, concurrency=1)
    requested: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        requested.append(request.headers["range"])
        return httpx.Response(
            206,
            content=body[4:],
            headers={"Content-Range": "bytes 4-9/10"},
            request=request,
        )

    async def run():
        downloader = HTTPDownloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_ranges(
                client, {}, part, state,
                {"total": len(body), "etag": '\"same-file\"', "last_modified": ""},
            )

    asyncio.run(run())
    assert requested == ["bytes=4-9"]
    assert part.read_bytes() == body
    saved = json.loads(state.read_text(encoding="utf-8"))
    assert saved["version"] == 3
    assert saved["resource_key"] == "https://cdn.test/file.mp4"
    assert "url" not in saved
    assert "token" not in state.read_text(encoding="utf-8")
    assert saved["ranges"][0]["current"] == 10


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


def test_http_range_downloader_accepts_server_capped_ranges(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    body = b"0123456789"
    task = Task(id="http-capped", url="https://files.test/capped.bin", task_type=TaskType.HTTP, concurrency=1)
    requests: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        value = request.headers["range"]
        requests.append(value)
        start_text, end_text = value.removeprefix("bytes=").split("-", 1)
        start, end = int(start_text), int(end_text)
        capped_end = min(end, start + 3)
        return httpx.Response(
            206,
            content=body[start : capped_end + 1],
            headers={"Content-Range": f"bytes {start}-{capped_end}/{len(body)}"},
            request=request,
        )

    async def run():
        downloader = HTTPDownloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_ranges(
                client,
                {"Accept-Encoding": "identity"},
                tmp_path / "payload.downloading",
                tmp_path / "resume.json",
                {"total": len(body), "etag": '"v1"', "last_modified": ""},
            )

    asyncio.run(run())
    assert (tmp_path / "payload.downloading").read_bytes() == body
    assert requests == ["bytes=0-9", "bytes=4-9", "bytes=8-9"]


def test_http_range_retry_resumes_after_mid_stream_disconnect(tmp_path, monkeypatch):
    from backend.app.downloader import http_file as http_file_module

    monkeypatch.setattr(http_file_module, "retry_delay_seconds", lambda *_args: 0)
    body = b"0123456789"
    task = Task(id="http-disconnect", url="https://files.test/disconnect.bin", task_type=TaskType.HTTP, concurrency=1)
    requests: list[str] = []

    class BrokenStream(httpx.AsyncByteStream):
        async def __aiter__(self):
            yield body[:4]
            raise httpx.ReadError("connection dropped")

        async def aclose(self):
            return None

    def handler(request: httpx.Request) -> httpx.Response:
        value = request.headers["range"]
        requests.append(value)
        start_text, end_text = value.removeprefix("bytes=").split("-", 1)
        start, end = int(start_text), int(end_text)
        if len(requests) == 1:
            return httpx.Response(
                206,
                stream=BrokenStream(),
                headers={"Content-Range": f"bytes {start}-{end}/{len(body)}"},
                request=request,
            )
        return httpx.Response(
            206,
            content=body[start : end + 1],
            headers={"Content-Range": f"bytes {start}-{end}/{len(body)}"},
            request=request,
        )

    async def run():
        downloader = HTTPDownloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_ranges(
                client,
                {"Accept-Encoding": "identity"},
                tmp_path / "payload.downloading",
                tmp_path / "resume.json",
                {"total": len(body), "etag": '"v1"', "last_modified": ""},
            )

    asyncio.run(run())
    assert (tmp_path / "payload.downloading").read_bytes() == body
    assert requests == ["bytes=0-9", "bytes=4-9"]


def test_http_run_falls_back_from_206_to_200_without_corrupting_sparse_offsets(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path / "downloads"))
    monkeypatch.setattr(settings, "temp_dir", str(tmp_path / "temp"))
    body = b"fallback-body"
    task = Task(id="http-fallback", url="https://files.test/download", task_type=TaskType.HTTP, concurrency=1)
    requests: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        value = request.headers.get("range", "")
        requests.append(value)
        if value == "bytes=0-255":
            return httpx.Response(
                206,
                content=body[:1],
                headers={"Content-Range": f"bytes 0-0/{len(body)}"},
                request=request,
            )
        if value:
            return httpx.Response(200, content=body, headers={"Content-Length": str(len(body))}, request=request)
        return httpx.Response(
            200,
            content=body,
            headers={"Content-Length": str(len(body)), "Content-Type": "application/pdf"},
            request=request,
        )

    class MockClient(httpx.AsyncClient):
        def __init__(self, *args, **kwargs):
            kwargs["transport"] = httpx.MockTransport(handler)
            super().__init__(*args, **kwargs)

    monkeypatch.setattr(http_file_module.httpx, "AsyncClient", MockClient)
    asyncio.run(HTTPDownloader(task).run())

    assert task.status.value == "done"
    assert Path(task.output_path).read_bytes() == body
    assert requests == ["bytes=0-255", "bytes=0-12", ""]
    assert Path(task.output_path).suffix == ".pdf"


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


def test_torrent_shutdown_preserves_payload_and_saves_resume(tmp_path, monkeypatch):
    from backend.app.downloader import torrent as torrent_module

    removed = []
    resume_saved = []

    class Status:
        has_metadata = False

    class Handle:
        def status(self):
            return Status()

        def pause(self):
            return None

    handle = Handle()

    class Session:
        def add_torrent(self, _params):
            return handle

        def remove_torrent(self, *args):
            removed.append(args)

    session = Session()

    class Params:
        save_path = ""

    class Libtorrent:
        @staticmethod
        def parse_magnet_uri(_url):
            return Params()

    monkeypatch.setattr(
        TorrentDownloader,
        "_load_libtorrent",
        staticmethod(lambda: Libtorrent),
    )
    monkeypatch.setattr(torrent_module, "_torrent_session", lambda _lt: session)
    task = Task(
        id="torrent-shutdown",
        url="magnet:?xt=urn:btih:0123456789abcdef",
        task_type=TaskType.TORRENT,
        engine_state={
            "temp_dir": str(tmp_path / "temp"),
            "output_dir": str(tmp_path / "out"),
        },
    )
    task.cancel_event = asyncio.Event()
    task.pause_event = asyncio.Event()
    downloader = TorrentDownloader(task)

    async def save_resume(_lt, _session, _handle, destination):
        resume_saved.append(destination)

    downloader._save_resume = save_resume

    async def run():
        runner = asyncio.create_task(downloader.run())
        await asyncio.sleep(0)
        runner.cancel()
        with pytest.raises(asyncio.CancelledError):
            await runner

    asyncio.run(run())

    assert resume_saved
    assert removed
    assert all(len(call) == 1 for call in removed)
    assert task.status is TaskStatus.PAUSED


def test_torrent_partial_selection_publishes_only_selected_files(tmp_path):
    class Storage:
        paths = ["bundle/keep.mp4", "bundle/skip.txt"]

        def num_files(self):
            return len(self.paths)

        def file_path(self, index):
            return self.paths[index]

    class Info:
        def name(self):
            return "bundle"

        def files(self):
            return Storage()

    payload = tmp_path / "temp" / ".tasks" / "torrent-select" / "payload"
    (payload / "bundle").mkdir(parents=True)
    (payload / "bundle" / "keep.mp4").write_bytes(b"selected")
    (payload / "bundle" / "skip.txt").write_bytes(b"unselected")
    task = Task(
        id="torrent-select",
        url="magnet:?xt=urn:btih:0123456789abcdef",
        task_type=TaskType.TORRENT,
        engine_state={
            "selected_files": [0],
            "temp_dir": str(tmp_path / "temp"),
            "output_dir": str(tmp_path / "out"),
        },
    )

    destination = TorrentDownloader(task)._move_payload(Info(), payload)

    assert destination.is_file()
    assert destination.name == "keep.mp4"
    assert destination.read_bytes() == b"selected"
    assert not (tmp_path / "out" / "skip.txt").exists()
