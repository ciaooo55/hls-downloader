import asyncio

import httpx
import pytest
from fastapi.testclient import TestClient

from backend.app.main import app
from backend.app.url_recognition import (
    MAX_CANDIDATES,
    RecognitionError,
    extract_html_candidates,
    recognize_url,
)


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


def test_treats_large_direct_archive_as_file_without_page_size_error():
    def handler(request: httpx.Request):
        return httpx.Response(
            200,
            headers={"Content-Type": "application/zip", "Content-Length": str(100 * 1024 * 1024)},
            content=b"not read as a page",
            request=request,
        )

    result = run_recognition("http://ipv4.download.test/100MB.zip", handler)

    assert result.kind == "file"
    assert result.candidates[0].source == "file"


def test_follows_redirect_before_returning_direct_file_candidate():
    def handler(request: httpx.Request):
        if request.url.host == "mirror.test":
            return httpx.Response(302, headers={"Location": "http://cdn.test/releases/system.iso"}, request=request)
        return httpx.Response(200, headers={"Content-Type": "application/octet-stream"}, content=b"iso", request=request)

    result = run_recognition("https://mirror.test/releases/system.iso", handler)

    assert result.kind == "file"
    assert result.final_url == "http://cdn.test/releases/system.iso"


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


def test_deduplicates_rotating_signatures_but_preserves_meaningful_query_values():
    html = """
    <script>
      const sources = [
        "https://cdn.test/watch.m3u8?id=episode-1&token=first&expires=100",
        "https://cdn.test/watch.m3u8?expires=200&id=episode-1&token=second",
        "https://cdn.test/watch.m3u8?id=episode-2&token=third"
      ];
    </script>
    """

    candidates = extract_html_candidates(html, "https://site.test/watch")

    assert [candidate.url for candidate in candidates] == [
        "https://cdn.test/watch.m3u8?id=episode-1&token=first&expires=100",
        "https://cdn.test/watch.m3u8?id=episode-2&token=third",
    ]


def test_prefers_master_over_renditions_from_the_same_quality_family():
    html = """
    <script>
      const low = "/hls/movie-360p.m3u8";
      const high = "/hls/movie-1080p.m3u8";
      const auto = "/hls/movie-master.m3u8";
    </script>
    """

    candidates = extract_html_candidates(html, "https://site.test/watch")

    assert len(candidates) == 1
    assert candidates[0].url == "https://site.test/hls/movie-master.m3u8"
    assert candidates[0].label == "主播放清单"
    assert candidates[0].quality == "master"
    assert 0.0 < candidates[0].confidence <= 1.0


def test_keeps_highest_rendition_and_does_not_merge_unrelated_families():
    html = """
    <script>
      const movieLow = "/hls/movie_480p.m3u8";
      const movieHigh = "/hls/movie_1080p.m3u8";
      const trailer = "/hls/trailer_720p.m3u8";
    </script>
    """

    candidates = extract_html_candidates(html, "https://site.test/watch")

    assert [(candidate.url, candidate.quality) for candidate in candidates] == [
        ("https://site.test/hls/movie_1080p.m3u8", "1080p"),
        ("https://site.test/hls/trailer_720p.m3u8", "720p"),
    ]


def test_filters_obvious_non_video_placeholders_and_extension_prefixes():
    html = r"""
    <script>
      const audio = "/tracks/audio/master.m3u8";
      const captions = "/tracks/subtitles-en.m3u8";
      const advertisement = "/ads/preroll.m3u8";
      const template = "/hls/${quality}.m3u8";
      const concatenated = "/hls/" + quality + ".m3u8";
      const javascript = "/assets/player.m3u8.js";
      const video = "https:\/\/cdn.test\/video\/master.m3u8?token=ok\u0026expires=2";
    </script>
    """

    candidates = extract_html_candidates(html, "https://site.test/watch")

    assert [candidate.url for candidate in candidates] == [
        "https://cdn.test/video/master.m3u8?token=ok&expires=2",
    ]


def test_candidate_count_is_capped_after_reduction():
    html = "<script>" + "\n".join(
        f'const stream{i} = "https://cdn.test/title-{i}/master.m3u8";'
        for i in range(MAX_CANDIDATES + 20)
    ) + "</script>"

    candidates = extract_html_candidates(html, "https://site.test/watch", limit=10_000)

    assert len(candidates) == MAX_CANDIDATES
    assert candidates[0].url == "https://cdn.test/title-0/master.m3u8"
    assert candidates[-1].url == f"https://cdn.test/title-{MAX_CANDIDATES - 1}/master.m3u8"


def test_marks_direct_multivariant_playlist_with_frontend_metadata():
    def handler(request: httpx.Request):
        return httpx.Response(
            200,
            headers={"Content-Type": "application/vnd.apple.mpegurl"},
            text="#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000,RESOLUTION=1280x720\n720.m3u8\n",
            request=request,
        )

    result = run_recognition("https://media.test/play", handler)

    assert result.kind == "hls"
    assert result.candidates[0].source == "playlist"
    assert result.candidates[0].label == "主播放清单"
    assert result.candidates[0].quality == "master"
    assert result.candidates[0].confidence == 1.0


def test_recognizes_direct_dash_manifest_by_mime_and_xml_signature():
    def handler(request: httpx.Request):
        return httpx.Response(
            200,
            headers={"Content-Type": "application/dash+xml"},
            text='<?xml version="1.0"?><MPD type="static"></MPD>',
            request=request,
        )

    result = run_recognition("https://media.test/manifest?id=1", handler)

    assert result.kind == "dash"
    assert result.candidates[0].source == "dash"
    assert result.candidates[0].label == "DASH 播放清单"
    assert result.candidates[0].quality == "dash"


def test_extracts_dash_from_page_and_filters_dash_audio_tracks():
    html = """
    <video data-manifest="/video/main.mpd?token=abc"></video>
    <script>const audio = '/tracks/audio.mpd'; const video = '/video/backup.mpd';</script>
    """

    candidates = extract_html_candidates(html, "https://site.test/watch")

    assert [candidate.url for candidate in candidates] == [
        "https://site.test/video/main.mpd?token=abc",
        "https://site.test/video/backup.mpd",
    ]
    assert candidates[0].label == "DASH 播放清单"


def test_reports_no_candidate_for_page_without_static_hls():
    def handler(request: httpx.Request):
        return httpx.Response(200, headers={"Content-Type": "text/html"}, text="<video src='blob:abc'>", request=request)

    result = run_recognition("https://site.test/watch/1", handler)

    assert result.kind == "none"
    assert result.candidates == []
    assert "浏览器插件" in result.message


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
