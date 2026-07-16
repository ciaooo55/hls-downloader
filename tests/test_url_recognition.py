import asyncio

import httpx
import pytest
from fastapi.testclient import TestClient

from backend.app.main import app
from backend.app.url_recognition import RecognitionError, recognize_url


def run_recognition(url: str, handler):
    async def run():
        transport = httpx.MockTransport(handler)
        async with httpx.AsyncClient(transport=transport, follow_redirects=True) as client:
            return await recognize_url(url, headers={}, client=client)

    return asyncio.run(run())


def test_recognizes_extensionless_playlist_from_signature():
    def handler(request: httpx.Request):
        return httpx.Response(
            200,
            headers={"Content-Type": "application/octet-stream"},
            text="#EXTM3U\n#EXT-X-TARGETDURATION:4\nsegment.ts\n",
            request=request,
        )

    result = run_recognition("https://media.test/play?id=1", handler)

    assert result.kind == "hls"
    assert [item.url for item in result.candidates] == ["https://media.test/play?id=1"]


def test_extracts_resolves_and_deduplicates_page_candidates():
    html = """
    <html><body>
      <video src="/hls/master.m3u8?token=abc"></video>
      <div data-stream="/hls/master.m3u8?token=abc"></div>
      <script>window.source = "../video/alt.m3u8";</script>
    </body></html>
    """

    def handler(request: httpx.Request):
        return httpx.Response(200, headers={"Content-Type": "text/html"}, text=html, request=request)

    result = run_recognition("https://site.test/watch/episode/1", handler)

    assert result.kind == "page"
    assert [item.url for item in result.candidates] == [
        "https://site.test/hls/master.m3u8?token=abc",
        "https://site.test/watch/video/alt.m3u8",
    ]


def test_reports_no_candidate_for_page_without_static_hls():
    def handler(request: httpx.Request):
        return httpx.Response(200, headers={"Content-Type": "text/html"}, text="<video src='blob:abc'>", request=request)

    result = run_recognition("https://site.test/watch/1", handler)

    assert result.kind == "none"
    assert result.candidates == []
    assert "ScriptCat" in result.message


def test_rejects_response_over_size_limit():
    def handler(request: httpx.Request):
        return httpx.Response(200, content=b"x" * (4 * 1024 * 1024 + 1), request=request)

    with pytest.raises(RecognitionError, match="4 MiB"):
        run_recognition("https://site.test/large", handler)


def test_recognition_api_requires_authentication(monkeypatch):
    async def fake_recognize(url, headers, client=None):
        class Result:
            def model_dump(self):
                return {"kind": "none", "final_url": url, "candidates": [], "message": "none"}

        return Result()

    from backend.app import api as api_module

    monkeypatch.setattr(api_module, "recognize_url", fake_recognize, raising=False)
    with TestClient(app) as client:
        unauthorized = client.post("/api/recognize", json={"url": "https://site.test/watch"})

    assert unauthorized.status_code == 401
