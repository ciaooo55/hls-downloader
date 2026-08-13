import asyncio

import httpx
import pytest

from backend.app.config import settings
from backend.app.downloader.http_file import HTTPDownloader
from backend.app.downloader.mirrors import (
    MAX_MIRRORS,
    canonical_http_url,
    mirror_identity_compatible,
    normalize_mirror_urls,
)
from backend.app.downloader.task_manager import TaskManager
from backend.app.models import Task, TaskType
from backend.app.schemas import TaskCreate


async def _async_noop(*_args, **_kwargs):
    return None


def test_normalize_mirror_urls_dedups_and_drops_primary():
    primary = "https://cdn.example.test/file.bin"
    mirrors = [
        "https://cdn.example.test/file.bin",
        "https://mirror.example.test/file.bin",
        "HTTPS://MIRROR.example.test/file.bin",
        "ftp://files.example.test/file.bin",
        "magnet:?xt=urn:btih:abc",
        "not-a-url",
        "# comment",
        "  https://backup.example.test/file.bin?token=1  ",
        "",
    ]
    assert normalize_mirror_urls(primary, mirrors) == [
        "https://mirror.example.test/file.bin",
        "https://backup.example.test/file.bin?token=1",
    ]
    assert normalize_mirror_urls(primary, "") == []
    assert normalize_mirror_urls(primary, None) == []


def test_normalize_mirror_urls_caps_count_and_ignores_same_default_port():
    primary = "https://files.example.test/a.bin"
    extras = [f"https://m{index}.example.test/a.bin" for index in range(20)]
    extras.append("https://files.example.test:443/a.bin")
    result = normalize_mirror_urls(primary, extras)
    assert len(result) == MAX_MIRRORS
    assert all("files.example.test" not in item for item in result)


def test_canonical_http_url_normalizes_host_and_default_port():
    assert canonical_http_url("HTTPS://CDN.Example.TEST:443/File.BIN?x=1#z") == (
        "https://cdn.example.test/File.BIN?x=1"
    )
    assert canonical_http_url("magnet:?xt=urn:btih:abc") == ""


def test_mirror_identity_requires_matching_size():
    primary = {"total": 100, "ranges": True, "etag": '"v1"', "last_modified": "now"}
    ok, reason = mirror_identity_compatible(primary, {"total": 99, "ranges": True, "etag": '"v1"'})
    assert ok is False
    assert "长度不一致" in reason


def test_mirror_identity_accepts_matching_etag_or_ranged_same_size():
    primary = {"total": 100, "ranges": True, "etag": '"abc"', "last_modified": ""}
    assert mirror_identity_compatible(primary, {"total": 100, "ranges": True, "etag": '"abc"'})[0] is True
    assert mirror_identity_compatible(primary, {"total": 100, "ranges": False, "etag": '"xyz"'})[0] is False
    assert mirror_identity_compatible(
        {"total": 100, "ranges": True, "etag": "", "last_modified": ""},
        {"total": 100, "ranges": True, "etag": "", "last_modified": ""},
    ) == (True, "size_range")


def test_mirror_identity_checksum_allows_etag_mismatch_of_same_size():
    primary = {"total": 50, "ranges": False, "etag": '"a"', "last_modified": "old"}
    candidate = {"total": 50, "ranges": False, "etag": '"b"', "last_modified": "new"}
    assert mirror_identity_compatible(primary, candidate)[0] is False
    assert mirror_identity_compatible(primary, candidate, has_checksum=True) == (True, "checksum")


def test_task_create_normalizes_mirrors_against_primary():
    body = TaskCreate(
        url="https://cdn.example.test/file.bin",
        mirrors=[
            "https://cdn.example.test/file.bin",
            "https://mirror.example.test/file.bin",
            "ftp://old.example.test/file.bin",
        ],
    )
    assert body.mirrors == ["https://mirror.example.test/file.bin"]


def test_task_create_rejects_mirrors_on_magnet():
    with pytest.raises(ValueError, match="magnet"):
        TaskCreate(
            url="magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
            mirrors=["https://cdn.example.test/a.bin"],
        )


def test_create_task_stores_mirrors_in_engine_state(monkeypatch):
    async def run():
        manager = TaskManager()
        monkeypatch.setattr(manager, "_save_db", _async_noop)
        monkeypatch.setattr("backend.app.downloader.task_manager.run_db", _async_noop)
        task = await manager.create_task(
            "https://cdn.example.test/file.bin",
            task_type=TaskType.HTTP,
            mirrors=["https://mirror.example.test/file.bin", "https://cdn.example.test/file.bin"],
        )
        assert task.engine_state["mirrors"] == ["https://mirror.example.test/file.bin"]
        event = manager._task_event(task)
        assert event["mirrors"] == ["https://mirror.example.test/file.bin"]
        assert event["mirror_status"] == []

    asyncio.run(run())


def _range_handler(body: bytes, etag: str = '"v1"'):
    def handler(request: httpx.Request) -> httpx.Response:
        value = request.headers.get("range", "")
        start_text, end_text = value.removeprefix("bytes=").split("-", 1)
        start, end = int(start_text), int(end_text)
        return httpx.Response(
            206,
            content=body[start : end + 1],
            headers={
                "Content-Range": f"bytes {start}-{end}/{len(body)}",
                "ETag": etag,
                "Content-Type": "application/octet-stream",
            },
            request=request,
        )
    return handler


def test_http_mirrors_empty_list_does_not_probe_extra_hosts(tmp_path, monkeypatch):
    body = b"0123456789abcdef" * 64
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    seen: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(str(request.url))
        return _range_handler(body)(request)

    task = Task(id="no-mirrors", url="https://files.test/video.bin", task_type=TaskType.HTTP, concurrency=2)

    async def run():
        part = tmp_path / "payload.downloading"
        downloader = HTTPDownloader(task)
        downloader._download_url = task.url
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_ranges(
                client,
                {},
                part,
                tmp_path / "resume.json",
                {"total": len(body), "etag": '"v1"', "last_modified": ""},
            )
        assert part.read_bytes() == body

    asyncio.run(run())
    assert seen
    assert all(url.startswith("https://files.test/video.bin") for url in seen)


def test_http_mirrors_range_workers_use_compatible_sources(tmp_path, monkeypatch):
    body = b"0123456789abcdef" * 131072 + b"tail"
    monkeypatch.setattr(settings, "http_chunk_size_mb", 1)
    seen: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(str(request.url.host))
        return _range_handler(body)(request)

    task = Task(
        id="multi-source",
        url="https://primary.test/file.bin",
        task_type=TaskType.HTTP,
        concurrency=4,
        engine_state={"mirrors": ["https://mirror.test/file.bin"]},
    )

    async def run():
        part = tmp_path / "payload.downloading"
        downloader = HTTPDownloader(task)
        downloader._install_source(
            {
                "total": len(body),
                "ranges": True,
                "etag": '"v1"',
                "last_modified": "",
                "final_url": "https://primary.test/file.bin",
            },
            origin_url=task.url,
        )
        downloader._install_source(
            {
                "total": len(body),
                "ranges": True,
                "etag": '"v1"',
                "last_modified": "",
                "final_url": "https://mirror.test/file.bin",
            },
            origin_url="https://mirror.test/file.bin",
        )
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            await downloader._download_ranges(
                client,
                {},
                part,
                tmp_path / "resume.json",
                {"total": len(body), "etag": '"v1"', "last_modified": ""},
            )
        assert part.read_bytes() == body

    asyncio.run(run())
    assert "primary.test" in seen
    assert "mirror.test" in seen


def test_http_mirrors_failover_when_primary_probe_fails():
    body = b"mirror-body-contents-ok!!"
    task = Task(
        id="failover",
        url="https://dead.test/file.bin",
        task_type=TaskType.HTTP,
        filename="file.bin",
        concurrency=2,
        engine_state={"mirrors": ["https://alive.test/file.bin"]},
    )

    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.host == "dead.test":
            return httpx.Response(403, text="expired", request=request)
        value = request.headers.get("range", "")
        if value:
            start_text, end_text = value.removeprefix("bytes=").split("-", 1)
            start, end = int(start_text), int(end_text)
            return httpx.Response(
                206,
                content=body[start : end + 1],
                headers={
                    "Content-Range": f"bytes {start}-{end}/{len(body)}",
                    "ETag": '"ok"',
                    "Content-Type": "application/octet-stream",
                    "Content-Disposition": 'attachment; filename="file.bin"',
                },
                request=request,
            )
        return httpx.Response(
            200,
            content=body,
            headers={
                "Content-Length": str(len(body)),
                "ETag": '"ok"',
                "Content-Type": "application/octet-stream",
            },
            request=request,
        )

    async def run():
        downloader = HTTPDownloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            metadata = await downloader._probe_metadata_with_failover(client, {})
            assert str(metadata["final_url"]).startswith("https://alive.test/")
            assert any(
                item.get("state") == "active" and "alive.test" in item.get("url", "")
                for item in task.engine_state.get("mirror_status", [])
            )

    asyncio.run(run())


def test_http_mirrors_skip_different_size_during_discovery():
    primary = {"total": 32, "ranges": True, "etag": '"p"', "last_modified": "", "final_url": "https://a.test/a.bin"}
    other = {"total": 16, "ranges": True, "etag": '"p"', "last_modified": "", "final_url": "https://b.test/b.bin"}
    task = Task(
        id="skip-size",
        url="https://a.test/a.bin",
        task_type=TaskType.HTTP,
        engine_state={"mirrors": ["https://b.test/b.bin"]},
    )
    downloader = HTTPDownloader(task)
    downloader._install_source(primary, origin_url=task.url)
    accepted, reason = downloader._accept_mirror_metadata("https://b.test/b.bin", other, primary)
    assert accepted is False
    assert "长度不一致" in reason
