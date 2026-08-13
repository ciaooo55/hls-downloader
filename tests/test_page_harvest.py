import asyncio

import httpx
import pytest
from fastapi.testclient import TestClient

from backend.app.main import app
from backend.app.page_harvest import (
    MAX_HARVEST_LINKS,
    HarvestError,
    extract_page_links,
    harvest_page,
    normalize_harvest_extensions,
    probe_harvest_links,
)


def run_harvest(url: str, handler, extensions=None):
    async def run():
        transport = httpx.MockTransport(handler)
        async with httpx.AsyncClient(transport=transport, follow_redirects=True) as client:
            return await harvest_page(url, headers={}, extensions=extensions, client=client)

    return asyncio.run(run())


def test_extracts_file_magnet_and_ftp_links_from_directory_listing():
    html = """
    <html><head><title>Night Files</title></head><body>
      <a href="movie.mp4">Film</a>
      <a href="javascript:alert(1)">bad</a>
      <a href="page.html">page</a>
      <a href="archive.zip" download="pack.zip">Zip pack</a>
      <img src="icon.png">
      <a href="../extra.mkv">Trailer</a>
      <a href="magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567">seed</a>
      <a href="ftp://nas.example.test/pub/keep.bin">keep</a>
    </body></html>
    """

    links, title, truncated = extract_page_links(html, "https://site.test/dir/")

    assert title == "Night Files"
    assert truncated is False
    assert [(item.filename, item.category, item.url) for item in links] == [
        ("movie.mp4", "video", "https://site.test/dir/movie.mp4"),
        ("pack.zip", "archive", "https://site.test/dir/archive.zip"),
        ("extra.mkv", "video", "https://site.test/extra.mkv"),
        ("seed", "torrent", "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"),
        ("keep.bin", "other", "ftp://nas.example.test/pub/keep.bin"),
    ]
    assert links[0].label == "Film"


def test_filters_by_requested_extensions_and_ignores_placeholders():
    html = """
    <a href="/files/a.mp4">A</a>
    <a href="/files/b.zip">B</a>
    <a href="/hls/${quality}.mp4">T</a>
    """

    links, _title, _truncated = extract_page_links(html, "https://site.test/", extensions=["zip"])

    assert [item.url for item in links] == ["https://site.test/files/b.zip"]


def test_harvest_link_count_is_capped_at_batch_limit():
    html = "".join(f'<a href="file-{index}.mp4">f{index}</a>' for index in range(MAX_HARVEST_LINKS + 8))

    links, _title, truncated = extract_page_links(html, "https://site.test/", limit=10_000)

    assert len(links) == MAX_HARVEST_LINKS
    assert truncated is True
    assert links[0].url == "https://site.test/file-0.mp4"
    assert links[-1].url == f"https://site.test/file-{MAX_HARVEST_LINKS - 1}.mp4"


def test_rejects_ftp_page_url_without_fetching():
    with pytest.raises(HarvestError, match=r"HTTP\(S\)"):
        run_harvest("ftp://nas.example.test/pub/", lambda request: httpx.Response(500, request=request))


def test_direct_file_is_not_harvested_as_a_page():
    def handler(request: httpx.Request):
        return httpx.Response(
            200,
            headers={"Content-Type": "application/zip", "Content-Length": "2048"},
            content=b"not a page",
            request=request,
        )

    result = run_harvest("https://cdn.test/app.zip", handler)

    assert result.kind == "file"
    assert result.links == []
    assert "\u65b0\u5efa\u4e0b\u8f7d" in result.message


def test_hls_playlist_is_not_harvested_as_a_page():
    def handler(request: httpx.Request):
        return httpx.Response(
            200,
            headers={"Content-Type": "application/vnd.apple.mpegurl"},
            text="#EXTM3U\n#EXT-X-TARGETDURATION:4\nsegment.ts\n",
            request=request,
        )

    result = run_harvest("https://media.test/master.m3u8", handler)

    assert result.kind == "hls"
    assert result.links == []


def test_harvests_one_html_page_only():
    html = "<html><head><title>Album</title></head><body><a href='/a.mp4'>A</a><a href='/b.mp3'>B</a></body></html>"

    def handler(request: httpx.Request):
        assert str(request.url) == "https://site.test/album"
        return httpx.Response(200, headers={"Content-Type": "text/html"}, text=html, request=request)

    result = run_harvest("https://site.test/album", handler)

    assert result.kind == "page"
    assert result.title == "Album"
    assert [item.url for item in result.links] == [
        "https://site.test/a.mp4",
        "https://site.test/b.mp3",
    ]
    assert result.truncated is False


def test_oversize_page_fails_closed():
    def handler(request: httpx.Request):
        return httpx.Response(200, content=b"x" * (4 * 1024 * 1024 + 1), request=request)

    with pytest.raises(HarvestError, match="4 MiB"):
        run_harvest("https://site.test/large", handler)


def test_empty_extensions_use_default_downloadable_set():
    assert "mp4" in normalize_harvest_extensions([])
    assert "png" not in normalize_harvest_extensions([])
    assert normalize_harvest_extensions(["PNG", ".Zip"]) == {"png", "zip"}


def test_harvest_api_requires_authentication(monkeypatch):
    async def fake_harvest(url, headers, extensions=None, client=None):
        class Result:
            def model_dump(self):
                return {"kind": "none", "page_url": url, "final_url": url, "title": "", "links": [], "truncated": False, "message": "none"}

        return Result()

    from backend.app import api as api_module

    monkeypatch.setattr(api_module, "harvest_page", fake_harvest, raising=False)
    with TestClient(app) as client:
        unauthorized = client.post("/api/recognize/harvest", json={"url": "https://site.test/album"})

    assert unauthorized.status_code == 401


def run_probe(urls, handler):
    async def run():
        transport = httpx.MockTransport(handler)
        async with httpx.AsyncClient(transport=transport, follow_redirects=True) as client:
            return await probe_harvest_links(urls, headers={}, client=client)

    return asyncio.run(run())


def test_probe_uses_head_length_and_range_total():
    def handler(request: httpx.Request):
        if str(request.url).endswith("/a.zip"):
            return httpx.Response(200, headers={"Content-Length": "4096"}, request=request)
        if request.method == "HEAD":
            return httpx.Response(200, headers={"Content-Type": "video/mp4"}, request=request)
        return httpx.Response(206, headers={"Content-Range": "bytes 0-0/81920"}, request=request)

    probes = run_probe([
        "https://cdn.example.test/a.zip",
        "https://cdn.example.test/movie.mp4",
        "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
    ], handler)
    by_url = {item.url: item for item in probes}
    assert by_url["https://cdn.example.test/a.zip"].size == 4096
    assert by_url["https://cdn.example.test/movie.mp4"].size == 81920
    assert by_url["magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"].ok is False


def test_probe_keeps_going_when_one_link_fails():
    def handler(request: httpx.Request):
        if "bad" in str(request.url):
            return httpx.Response(500, request=request)
        return httpx.Response(200, headers={"Content-Length": "12"}, request=request)

    probes = run_probe([
        "https://cdn.example.test/ok.bin",
        "https://cdn.example.test/bad.bin",
    ], handler)
    assert probes[0].size == 12
    assert probes[1].ok is False


def test_harvest_probe_api_requires_authentication():
    from backend.app.main import app
    from fastapi.testclient import TestClient

    with TestClient(app) as client:
        unauthorized = client.post("/api/recognize/harvest/probe", json={"urls": ["https://cdn.example.test/a.bin"]})
    assert unauthorized.status_code == 401

