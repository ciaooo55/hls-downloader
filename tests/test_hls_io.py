import asyncio
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
from backend.app.models import Task


def _task(url: str = "https://example.test/master.m3u8") -> Task:
    return Task(id="test", url=url, filename="video")


def test_browser_transport_matches_the_captured_user_agent_family():
    assert _browser_impersonation({"User-Agent": "Mozilla/5.0 Chrome/140.0 Safari/537.36"}) == "chrome"
    assert _browser_impersonation({"user-agent": "Mozilla/5.0 Firefox/152.0"}) == "firefox"
    assert _browser_impersonation({"User-Agent": "Mozilla/5.0 Version/18.0 Safari/605.1.15"}) == "safari"


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
            "default_headers": False,
            "http_version": "v1",
            "timeout": (10, 60),
            "allow_redirects": True,
        }
    ]
    assert requested[0]["impersonate"] == "chrome"
