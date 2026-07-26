import asyncio
from pathlib import Path

import httpx
import pytest

from backend.app.downloader import dash_native as native_module
from backend.app.downloader.dash_native import NativeDashEngine
from backend.app.downloader.mpd import parse_mpd
from backend.app.downloader.parser import list_hls_video_tracks
from backend.app.models import Task, TaskStatus


MPD_NS = 'xmlns="urn:mpeg:dash:schema:mpd:2011"'
MULTI_TRACK_MPD = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT8S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="$RepresentationID$/$Number$.m4s"
        initialization="$RepresentationID$/init.mp4" duration="4" timescale="1"/>
      <Representation id="v1080" bandwidth="5000000" width="1920" height="1080"/>
      <Representation id="v720" bandwidth="2500000" width="1280" height="720"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="zh">
      <SegmentTemplate media="zh/$Number$.m4s" initialization="zh/init.mp4"
        duration="4" timescale="1"/>
      <Representation id="a-zh" bandwidth="128000"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="en">
      <SegmentTemplate media="en/$Number$.m4s" initialization="en/init.mp4"
        duration="4" timescale="1"/>
      <Representation id="a-en" bandwidth="192000"/>
    </AdaptationSet>
  </Period>
</MPD>"""


def test_parse_mpd_lists_options_and_defaults_to_best():
    parsed = parse_mpd("https://cdn.test/main.mpd", MULTI_TRACK_MPD)
    assert [option["id"] for option in parsed["video_options"]] == ["v1080", "v720"]
    assert {option["lang"] for option in parsed["audio_options"]} == {"zh", "en"}
    # Automatic pick: highest resolution video, highest bandwidth audio.
    assert parsed["video"]["id"] == "v1080"
    assert parsed["audio"]["id"] == "a-en"


def test_parse_mpd_honors_preferred_video_and_audio_language():
    parsed = parse_mpd(
        "https://cdn.test/main.mpd",
        MULTI_TRACK_MPD,
        preferred_video="v720",
        preferred_audio="zh",
    )
    assert parsed["video"]["id"] == "v720"
    assert parsed["audio"]["id"] == "a-zh"


def test_parse_mpd_falls_back_when_preference_is_stale():
    parsed = parse_mpd(
        "https://cdn.test/main.mpd",
        MULTI_TRACK_MPD,
        preferred_video="gone",
        preferred_audio="jp",
    )
    assert parsed["video"]["id"] == "v1080"
    assert parsed["audio"]["id"] == "a-en"


def test_list_hls_video_tracks_orders_and_skips_audio_only():
    master = """#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS="avc1.42c01e,mp4a.40.2"
low.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080,CODECS="avc1.640028,mp4a.40.2"
high.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=192000,CODECS="mp4a.40.2"
audio.m3u8
"""
    tracks = list_hls_video_tracks("https://cdn.test/master.m3u8", master)
    assert [track["height"] for track in tracks] == [1080, 360]
    assert tracks[0]["id"] == "https://cdn.test/high.m3u8"
    assert all("audio.m3u8" not in track["id"] for track in tracks)


def _task(tmp_path: Path) -> Task:
    task = Task(id="select-test", url="https://cdn.test/main.mpd", filename="剧集")
    task.cancel_event = asyncio.Event()
    task.pause_event = asyncio.Event()
    task.engine_state["temp_dir"] = str(tmp_path / "temp")
    task.engine_state["output_dir"] = str(tmp_path / "out")
    return task


def test_native_engine_downloads_selected_tracks(tmp_path, monkeypatch):
    requested: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            return httpx.Response(200, text=MULTI_TRACK_MPD)
        requested.append(target)
        return httpx.Response(200, content=b"x" * 8)

    real_client = httpx.AsyncClient

    def fake_client(**kwargs):
        return real_client(
            transport=httpx.MockTransport(handler), follow_redirects=True
        )

    monkeypatch.setattr(native_module.httpx, "AsyncClient", fake_client)

    async def fake_ffmpeg(command, task=None, duration_sec=0, on_progress=None):
        Path(command[-1]).write_bytes(b"muxed")
        return True

    async def fake_verify(ffmpeg_path, output_path, expected_duration):
        return None

    monkeypatch.setattr(native_module, "_run_ffmpeg", fake_ffmpeg)
    monkeypatch.setattr(native_module, "_verify_output", fake_verify)

    async def run():
        task = _task(tmp_path)
        task.selected_video = "v720"
        task.selected_audio = "zh"
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert any("/v720/" in url for url in requested)
        assert not any("/v1080/" in url for url in requested)
        assert any("/zh/" in url for url in requested)
        assert not any("/en/" in url for url in requested)

    asyncio.run(run())


MASTER_WITH_SEPARATE_AUDIO = """#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="chinese",DEFAULT=YES,URI="audio.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,AUDIO="aud"
low.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080,AUDIO="aud"
high.m3u8
"""


def test_selected_rendition_keeps_external_audio_detection():
    """Selecting a rendition must not bypass the master (silent video)."""
    from backend.app.downloader.parser import parse_m3u8

    parsed = parse_m3u8(
        "https://cdn.test/master.m3u8",
        MASTER_WITH_SEPARATE_AUDIO,
        preferred_variant="https://cdn.test/low.m3u8",
    )
    assert parsed["type"] == "variant"
    assert parsed["url"] == "https://cdn.test/low.m3u8"
    # The separate audio rendition is still reported, so the engine routes
    # to the muxing path instead of downloading video without sound.
    assert parsed["external_audio"] is True


def test_stale_rendition_selection_falls_back_to_best():
    from backend.app.downloader.parser import parse_m3u8

    parsed = parse_m3u8(
        "https://cdn.test/master.m3u8",
        MASTER_WITH_SEPARATE_AUDIO,
        preferred_variant="https://cdn.test/gone.m3u8",
    )
    assert parsed["url"] == "https://cdn.test/high.m3u8"
