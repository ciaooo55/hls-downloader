import asyncio
import types
from pathlib import Path

import httpx
import pytest
from cryptography.hazmat.primitives import padding
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes

from backend.app.config import settings
from backend.app.downloader.hls import (
    _browser_impersonation,
    HLSDownloader,
    _create_hls_client,
    _decrypt_aes128_file,
    _reserve_output_path,
)
from backend.app.downloader.errors import diagnose_download_error
from backend.app.downloader.playback import playback_service, write_playback_plan
from backend.app.downloader.progress import ProgressTracker
from backend.app.models import Task


def _task(url: str = "https://example.test/master.m3u8") -> Task:
    return Task(id="test", url=url, filename="video")


def test_browser_transport_uses_one_supported_coherent_profile():
    assert _browser_impersonation() == "chrome"


def test_hls_progress_estimates_total_from_completed_segments():
    tracker = ProgressTracker()
    tracker.start(10)
    tracker.add_completed(100)
    tracker.add_completed(100)

    snapshot = tracker.snapshot()

    assert snapshot["downloaded_bytes"] == 200
    assert snapshot["total_bytes"] == 1000


def test_hls_warms_a_contiguous_prefix_before_parallel_downloads(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    task = _task()
    task.status = task.status.DOWNLOADING_SEGMENTS
    task.progress.total_segments = 3
    downloader = HLSDownloader(task)
    task_dir = tmp_path / ".tasks" / task.id
    segments = [
        {"index": 0, "url": "https://example.test/0.ts", "duration": 0.5},
        {"index": 1, "url": "https://example.test/1.ts", "duration": 0.5},
        {"index": 2, "url": "https://example.test/2.ts", "duration": 4.0},
    ]
    write_playback_plan(task_dir, segments, total_duration=5.0)
    calls: list[str] = []

    class Client:
        async def download_to_file(self, url, destination, *_args):
            calls.append(url)
            destination.write_bytes(url.encode())
            return types.SimpleNamespace(status_code=200, headers={}), len(url)

    async def run():
        assert await downloader._download_segments(Client(), segments, {}, concurrency=3)

    asyncio.run(run())

    # The two 0.5-second segments are required to cross the one-second
    # playback threshold and must land before the normal worker pool starts.
    assert calls[:2] == ["https://example.test/0.ts", "https://example.test/1.ts"]
    snapshot = playback_service.snapshot(task.id, task.status.value)
    assert snapshot.ready is True
    assert snapshot.available_segments == 3
    assert task.progress.active_workers == 0


def test_load_media_playlist_follows_variants_and_rejects_cycles():
    responses = {
        "https://example.test/master.m3u8": """#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=100
low.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=1000
level/master.m3u8
""",
        "https://example.test/level/master.m3u8": """#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=2000
../media.m3u8
""",
        "https://example.test/media.m3u8": """#EXTM3U
#EXTINF:4,
one.ts
#EXT-X-ENDLIST
""",
    }

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, text=responses[str(request.url)])

    async def run():
        downloader = HLSDownloader(_task())
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = await downloader._load_media_playlist(
                client,
                "https://example.test/master.m3u8",
                {},
            )
        assert parsed["url"] == "https://example.test/media.m3u8"
        assert parsed["segments"][0]["url"] == "https://example.test/one.ts"

    asyncio.run(run())


def test_load_media_playlist_retries_a_transient_origin_failure(monkeypatch):
    from backend.app.downloader import hls as hls_module

    calls = []
    monkeypatch.setattr(hls_module, "retry_delay_seconds", lambda *_args: 0)

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(str(request.url))
        if len(calls) == 1:
            return httpx.Response(503, request=request)
        return httpx.Response(
            200,
            request=request,
            text="#EXTM3U\n#EXTINF:4,\none.ts\n#EXT-X-ENDLIST\n",
        )

    async def run():
        downloader = HLSDownloader(_task())
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = await downloader._load_media_playlist(
                client,
                "https://example.test/master.m3u8",
                {},
            )
        assert len(parsed["segments"]) == 1

    asyncio.run(run())
    assert calls == [
        "https://example.test/master.m3u8",
        "https://example.test/master.m3u8",
    ]


def test_hls_control_request_retries_unrequested_transport_cancellation(monkeypatch):
    from backend.app.downloader import hls as hls_module

    monkeypatch.setattr(hls_module, "retry_delay_seconds", lambda *_args: 0)
    downloader = HLSDownloader(_task())
    calls = 0

    async def request():
        nonlocal calls
        calls += 1
        if calls == 1:
            # Models curl_cffi cancelling its internal stream task after a
            # transient TLS/socket failure.  No user event is set.
            raise asyncio.CancelledError
        return "recovered"

    async def run():
        assert await downloader._retry_control_request(
            request,
            stage="parsing",
            url="https://example.test/master.m3u8",
            label="HLS 清单",
        ) == "recovered"

    asyncio.run(run())
    assert calls == 2
    assert downloader.task.pause_event is None


def test_hls_segment_retries_unrequested_transport_cancellation(tmp_path, monkeypatch):
    from backend.app.downloader import hls as hls_module

    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    monkeypatch.setattr(hls_module, "retry_delay_seconds", lambda *_args: 0)
    downloader = HLSDownloader(_task("https://example.test/master.m3u8"))
    calls = 0

    class Client:
        async def download_to_file(
            self,
            _url,
            destination,
            _headers,
            _cancel_check,
            _task,
        ):
            nonlocal calls
            calls += 1
            if calls == 1:
                raise asyncio.CancelledError
            destination.write_bytes(b"segment")
            return types.SimpleNamespace(status_code=200, headers={}), 7

    async def run():
        assert await downloader._download_one_segment(
            Client(),
            {"index": 0, "url": "https://example.test/one.ts"},
            {},
        )

    asyncio.run(run())
    assert calls == 2
    assert downloader.task.progress.reconnect_count == 1
    assert (tmp_path / ".tasks" / "test" / "segments" / "000000.seg").read_bytes() == b"segment"


def test_vod_resume_reuses_complete_segment_across_signature_refresh_without_leaking_url(
    tmp_path, monkeypatch
):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    old = {
        "index": 0,
        "url": "https://edge-a.test/media/part.m4s?quality=1080&token=old-secret",
        "duration": 4.0,
        "media_sequence": 10,
        "byte_range": None,
        "key": None,
        "init_map": {
            "uri": "https://edge-a.test/media/init.mp4?token=old-secret",
            "byte_range": None,
        },
    }
    first = HLSDownloader(_task())
    first._prepare_vod_resume([old])
    path = first._seg_dir() / "000000.seg"
    path.write_bytes(b"complete-segment")
    asyncio.run(first._checkpoint_vod_segment(0, path.stat().st_size))

    refreshed = dict(old)
    refreshed["url"] = (
        "https://edge-b.test/media/part.m4s?token=new-secret&quality=1080"
    )
    refreshed["init_map"] = {
        "uri": "https://edge-b.test/media/init.mp4?token=new-secret",
        "byte_range": None,
    }
    second = HLSDownloader(_task())
    second._prepare_vod_resume([refreshed])

    assert path.read_bytes() == b"complete-segment"
    checkpoint = second._vod_state_path().read_text(encoding="utf-8")
    assert "old-secret" not in checkpoint
    assert "new-secret" not in checkpoint
    assert "https://" not in checkpoint


@pytest.mark.parametrize(
    "changed",
    [
        {"url": "https://edge.test/media/part.m4s?quality=720&token=new"},
        {"byte_range": {"offset": 100, "length": 50}},
        {
            "key": {
                "uri": "https://edge.test/keys/other.bin?token=new",
                "iv": b"\x01" * 16,
            }
        },
        {
            "init_map": {
                "uri": "https://edge.test/media/other-init.mp4?token=new",
                "byte_range": None,
            }
        },
    ],
)
def test_vod_resume_discards_segment_when_byte_identity_changes(
    tmp_path, monkeypatch, changed
):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    original = {
        "index": 0,
        "url": "https://edge.test/media/part.m4s?quality=1080&token=old",
        "duration": 4.0,
        "media_sequence": 10,
        "byte_range": {"offset": 0, "length": 50},
        "key": {
            "uri": "https://edge.test/keys/main.bin?token=old",
            "iv": b"\x01" * 16,
        },
        "init_map": {
            "uri": "https://edge.test/media/init.mp4?token=old",
            "byte_range": None,
        },
    }
    first = HLSDownloader(_task())
    first._prepare_vod_resume([original])
    path = first._seg_dir() / "000000.seg"
    path.write_bytes(b"x" * 50)
    asyncio.run(first._checkpoint_vod_segment(0, 50))

    updated = dict(original)
    updated.update(changed)
    second = HLSDownloader(_task())
    second._prepare_vod_resume([updated])

    assert not path.exists()


def test_vod_resume_discards_truncated_and_uncheckpointed_files(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    segments = [
        {"index": 0, "url": "https://edge.test/0.ts", "duration": 4.0},
        {"index": 1, "url": "https://edge.test/1.ts", "duration": 4.0},
    ]
    first = HLSDownloader(_task())
    first._prepare_vod_resume(segments)
    complete = first._seg_dir() / "000000.seg"
    complete.write_bytes(b"complete")
    asyncio.run(first._checkpoint_vod_segment(0, complete.stat().st_size))
    complete.write_bytes(b"cut")
    orphan = first._seg_dir() / "000001.seg"
    orphan.write_bytes(b"not-checkpointed")
    (first._seg_dir() / "000002.seg.tmp").write_bytes(b"partial")

    second = HLSDownloader(_task())
    second._prepare_vod_resume(segments)

    assert not complete.exists()
    assert not orphan.exists()
    assert not (first._seg_dir() / "000002.seg.tmp").exists()


def test_hls_run_completes_after_an_unrequested_segment_cancellation(tmp_path, monkeypatch):
    from backend.app.downloader import hls as hls_module

    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    monkeypatch.setattr(hls_module, "retry_delay_seconds", lambda *_args: 0)
    attempts = 0

    class Client:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def download_to_file(
            self,
            _url,
            destination,
            _headers,
            _cancel_check,
            _task,
        ):
            nonlocal attempts
            attempts += 1
            if attempts == 1:
                raise asyncio.CancelledError
            destination.write_bytes(b"segment")
            return types.SimpleNamespace(status_code=200, headers={}), 7

    async def merge(*, output_path, **_kwargs):
        output_path.write_bytes(b"merged")

    async def verified(*_args, **_kwargs):
        return True

    async def playlist(_client, _url, _headers):
        return {
            "is_live": False,
            "segments": [{"index": 0, "url": "https://example.test/one.ts"}],
            "total_duration": 1.0,
            "content": "#EXTM3U\n#EXTINF:1,\none.ts\n#EXT-X-ENDLIST\n",
            "title": "",
            "response_filename": "",
            "final_url": "https://example.test/master.m3u8",
            "external_audio": False,
            "subtitle_tracks": [],
        }

    monkeypatch.setattr(hls_module, "_create_hls_client", lambda *_args: Client())
    monkeypatch.setattr(hls_module, "merge_segments", merge)
    monkeypatch.setattr(hls_module, "verify_task_checksum", verified)
    downloader = HLSDownloader(_task())
    downloader._load_media_playlist = playlist

    asyncio.run(downloader.run())

    assert attempts == 2
    assert downloader.task.status.value == "done"
    assert downloader.task.stage == "done"


def test_hls_control_resources_retry_transient_failures(tmp_path, monkeypatch):
    from backend.app.downloader import hls as hls_module

    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    monkeypatch.setattr(hls_module, "retry_delay_seconds", lambda *_args: 0)
    key = b"0123456789abcdef"
    calls = {"init": 0, "key": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        name = request.url.path.rsplit("/", 1)[-1]
        calls[name.split(".", 1)[0]] += 1
        if calls[name.split(".", 1)[0]] == 1:
            return httpx.Response(503, request=request)
        return httpx.Response(200, request=request, content=key if name == "key.bin" else b"init")

    async def run():
        downloader = HLSDownloader(_task())
        segment = {
            "index": 0,
            "init_map": {"uri": "https://example.test/init.mp4", "byte_range": None},
        }
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_init_maps(client, [segment], {})
            assert Path(segment["init_path"]).read_bytes() == b"init"
            assert await downloader._fetch_key(client, "https://example.test/key.bin", {}) == key

    asyncio.run(run())
    assert calls == {"init": 2, "key": 2}


def test_hls_with_external_audio_uses_adaptive_compatibility_engine(tmp_path, monkeypatch):
    from backend.app.downloader import hls as hls_module

    monkeypatch.setattr(settings, "download_dir", str(tmp_path))
    task = _task()
    calls = []

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

    class FakeAdaptiveDownloader:
        def __init__(self, received_task, on_progress=None, on_log=None, source_label=""):
            assert received_task is task
            assert source_label == "HLS 独立音轨"
            self.task = received_task
            self.on_progress = on_progress
            self.on_log = on_log

        async def run(self):
            calls.append("run")
            self.task.status = self.task.status.DONE

    async def fake_playlist(self, _client, _url, _headers):
        return {"external_audio": True}

    monkeypatch.setattr(hls_module, "_create_hls_client", lambda *_args: FakeClient())
    monkeypatch.setattr(hls_module, "DashDownloader", FakeAdaptiveDownloader)
    downloader = HLSDownloader(task)
    downloader._load_media_playlist = types.MethodType(fake_playlist, downloader)

    asyncio.run(downloader.run())

    assert calls == ["run"]
    assert task.stage == "parsing"
    assert "独立 HLS 音轨" in task.last_log


def test_download_resource_validates_byte_range_and_renames_atomically(tmp_path):
    body = b"0123456789"

    def good_handler(request: httpx.Request) -> httpx.Response:
        assert request.headers["range"] == "bytes=2-5"
        return httpx.Response(
            206,
            content=body[2:6],
            headers={"Content-Range": "bytes 2-5/10"},
        )

    async def run_good():
        destination = tmp_path / "part.seg"
        downloader = HLSDownloader(_task())
        async with httpx.AsyncClient(transport=httpx.MockTransport(good_handler)) as client:
            size = await downloader._download_resource(
                client,
                "https://example.test/media.bin",
                destination,
                {},
                {"offset": 2, "length": 4},
            )
        assert size == 4
        assert destination.read_bytes() == b"2345"
        assert not destination.with_suffix(".seg.tmp").exists()

    asyncio.run(run_good())

    def bad_handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            206,
            content=b"wrong",
            headers={"Content-Range": "bytes 0-4/10"},
        )

    async def run_bad():
        destination = tmp_path / "bad.seg"
        downloader = HLSDownloader(_task())
        async with httpx.AsyncClient(transport=httpx.MockTransport(bad_handler)) as client:
            with pytest.raises(Exception, match="Content-Range"):
                await downloader._download_resource(
                    client,
                    "https://example.test/media.bin",
                    destination,
                    {},
                    {"offset": 2, "length": 4},
                )
        assert not destination.exists()

    asyncio.run(run_bad())


def test_byte_range_http_error_keeps_real_status_code(tmp_path):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(403, text="forbidden")

    async def run():
        downloader = HLSDownloader(_task())
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            with pytest.raises(httpx.HTTPStatusError) as raised:
                await downloader._download_resource(
                    client,
                    "https://example.test/media.bin",
                    tmp_path / "forbidden.seg",
                    {},
                    {"offset": 2, "length": 4},
                )
        details = diagnose_download_error(
            raised.value,
            stage="downloading_segments",
            url="https://example.test/media.bin",
        )
        assert details.code == "HTTP_403"
        assert details.http_status == 403

    asyncio.run(run())


def test_decrypt_aes128_file_validates_and_removes_pkcs7_padding(tmp_path):
    key = b"0123456789abcdef"
    iv = (42).to_bytes(16, "big")
    plaintext = b"transport-stream-data"
    padder = padding.PKCS7(128).padder()
    padded = padder.update(plaintext) + padder.finalize()
    encryptor = Cipher(algorithms.AES(key), modes.CBC(iv)).encryptor()
    encrypted = encryptor.update(padded) + encryptor.finalize()

    source = tmp_path / "encrypted.bin"
    destination = tmp_path / "decrypted.seg"
    source.write_bytes(encrypted)

    _decrypt_aes128_file(source, destination, key, iv)

    assert destination.read_bytes() == plaintext


def test_download_init_map_decrypts_aes128_resource(tmp_path, monkeypatch):
    key = b"0123456789abcdef"
    iv = (7).to_bytes(16, "big")
    plaintext = b"fmp4-init-section"
    padder = padding.PKCS7(128).padder()
    padded = padder.update(plaintext) + padder.finalize()
    encryptor = Cipher(algorithms.AES(key), modes.CBC(iv)).encryptor()
    encrypted = encryptor.update(padded) + encryptor.finalize()
    monkeypatch.setattr(settings, "download_dir", str(tmp_path))

    def handler(request: httpx.Request) -> httpx.Response:
        if str(request.url) == "https://example.test/key.bin":
            return httpx.Response(200, content=key)
        return httpx.Response(200, content=encrypted)

    async def run():
        downloader = HLSDownloader(_task())
        segment = {
            "index": 0,
            "init_map": {"uri": "https://example.test/init.mp4", "byte_range": None},
            "key": {
                "method": "AES-128",
                "uri": "https://example.test/key.bin",
                "iv": iv,
            },
        }
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_init_maps(client, [segment], {})
        assert Path(segment["init_path"]).read_bytes() == plaintext

    asyncio.run(run())


def test_reserve_output_path_is_atomic(tmp_path):
    first = _reserve_output_path(tmp_path / "video.mp4")
    second = _reserve_output_path(tmp_path / "video.mp4")

    assert first.name == "video.mp4"
    assert second.name == "video_1.mp4"
    assert first.exists()
    assert second.exists()


def test_browser_transport_matches_request_tls_and_streams_to_disk(tmp_path, monkeypatch):
    from backend.app.downloader import hls as hls_module

    created = []
    requested = []

    class FakeResponse:
        status_code = 200
        headers = {"Content-Length": "6"}
        quit_now = None
        astream_task = None

        async def aiter_content(self):
            yield b"abc"
            yield b"def"

    class FakeSession:
        def __init__(self, **kwargs):
            created.append(kwargs)

        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def get(self, _url, **kwargs):
            assert kwargs["stream"] is True
            requested.append(kwargs)
            return FakeResponse()

    monkeypatch.setattr(hls_module, "CurlAsyncSession", FakeSession)

    async def run():
        client = _create_hls_client(4)
        downloader = HLSDownloader(_task())
        destination = tmp_path / "browser.seg"
        async with client:
            written = await downloader._download_resource(
                client,
                "https://example.test/browser.seg",
                destination,
                {"User-Agent": "Mozilla/5.0 Chrome/140.0 Safari/537.36"},
            )
        assert written == 6
        assert destination.read_bytes() == b"abcdef"

    asyncio.run(run())
    assert created == [
        {
            "max_clients": 8,
            "default_headers": True,
            "http_version": "v1",
            "timeout": (10, 60),
            "allow_redirects": False,
        }
    ]
    assert requested[0]["impersonate"] == "chrome"
    assert "user-agent" not in {name.lower() for name in requested[0]["headers"]}


def test_browser_transport_retries_cloudflare_403_without_stale_cf_cookie(monkeypatch):
    from backend.app.downloader import hls as hls_module

    requested = []

    class FakeResponse:
        def __init__(self, status_code):
            self.status_code = status_code
            self.closed = False

        async def aclose(self):
            self.closed = True

    first = FakeResponse(403)
    second = FakeResponse(200)

    class FakeSession:
        def __init__(self, **_kwargs):
            pass

        async def get(self, _url, **kwargs):
            requested.append(kwargs)
            return first if len(requested) == 1 else second

    monkeypatch.setattr(hls_module, "CurlAsyncSession", FakeSession)

    async def run():
        client = _create_hls_client(2)
        response = await client.get(
            "https://example.test/playlist.m3u8",
            headers={"Cookie": "session=ok; __cf_bm=expired; __cflb=stale"},
        )
        assert response is second

    asyncio.run(run())
    assert first.closed is True
    assert len(requested) == 2
    assert requested[0]["headers"]["Cookie"] == "session=ok; __cf_bm=expired; __cflb=stale"
    assert requested[1]["headers"]["Cookie"] == "session=ok"
    assert requested[0]["impersonate"] == requested[1]["impersonate"] == "chrome"
