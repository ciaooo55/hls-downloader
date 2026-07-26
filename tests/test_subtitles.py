import asyncio
from pathlib import Path

import httpx
import pytest

from backend.app.config import settings
from backend.app.downloader import hls as hls_module
from backend.app.downloader.hls import HLSDownloader
from backend.app.downloader.subtitles import (
    has_cues,
    merge_webvtt_segments,
    webvtt_to_srt,
)
from backend.app.models import Task, TaskStatus


def test_merge_applies_timestamp_map_offset_and_dedupes_boundary_cues():
    first = (
        "WEBVTT\n"
        "X-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:900000\n"
        "\n"
        "00:00.000 --> 00:02.000\n"
        "第一句\n"
        "\n"
        "00:02.000 --> 00:04.000\n"
        "第二句\n"
    )
    second = (
        "WEBVTT\n"
        "X-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:900000\n"
        "\n"
        "00:02.000 --> 00:04.000\n"
        "第二句\n"
        "\n"
        "00:04.000 --> 00:06.000\n"
        "第三句\n"
    )
    merged = merge_webvtt_segments([first, second])
    assert merged.startswith("WEBVTT")
    # MPEGTS 900000 / 90000 = 10 second offset applied to every cue.
    assert "00:00:10.000 --> 00:00:12.000" in merged
    assert "00:00:14.000 --> 00:00:16.000" in merged
    # The boundary cue repeated by the second segment appears exactly once.
    assert merged.count("第二句") == 1
    assert has_cues(merged)


def test_merge_without_timestamp_map_keeps_original_times():
    text = "WEBVTT\n\n00:00:01.500 --> 00:00:03.000\nhello\n"
    merged = merge_webvtt_segments([text])
    assert "00:00:01.500 --> 00:00:03.000" in merged


def test_webvtt_to_srt_strips_vtt_only_tags_and_uses_comma_decimals():
    vtt = (
        "WEBVTT\n"
        "\n"
        "00:00:01.000 --> 00:00:02.500 align:center\n"
        "<v Speaker><i>你好</i></v>\n"
        "\n"
        "00:00:03.000 --> 00:00:04.000\n"
        "<c.yellow>world</c>\n"
    )
    srt = webvtt_to_srt(vtt)
    assert "1\n00:00:01,000 --> 00:00:02,500\n<i>你好</i>" in srt
    assert "2\n00:00:03,000 --> 00:00:04,000\nworld" in srt
    assert "<v" not in srt and "<c" not in srt


VIDEO_PLAYLIST = (
    "#EXTM3U\n"
    "#EXT-X-TARGETDURATION:4\n"
    "#EXT-X-MEDIA-SEQUENCE:0\n"
    "#EXTINF:4,\nv0.ts\n"
    "#EXTINF:4,\nv1.ts\n"
    "#EXT-X-ENDLIST\n"
)
SUBS_PLAYLIST = (
    "#EXTM3U\n"
    "#EXT-X-TARGETDURATION:4\n"
    "#EXT-X-MEDIA-SEQUENCE:0\n"
    "#EXTINF:4,\nzh0.vtt\n"
    "#EXTINF:4,\nzh1.vtt\n"
    "#EXT-X-ENDLIST\n"
)
MASTER_PLAYLIST = (
    "#EXTM3U\n"
    '#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="中文",LANGUAGE="zh",'
    'DEFAULT=YES,URI="subs_zh.m3u8"\n'
    '#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=640x360,SUBTITLES="subs"\n'
    "video.m3u8\n"
)


def _task(tmp_path: Path, url: str) -> Task:
    task = Task(id="subs-test", url=url, filename="剧集")
    task.cancel_event = asyncio.Event()
    task.pause_event = asyncio.Event()
    task.engine_state["temp_dir"] = str(tmp_path / "temp")
    task.engine_state["output_dir"] = str(tmp_path / "out")
    return task


def _install_fake_merge(monkeypatch):
    async def fake_merge(*, seg_dir, output_path, segments, **kwargs):
        payload = b"".join(
            (seg_dir / f"{segment['index']:06d}.seg").read_bytes()
            for segment in segments
        )
        Path(output_path).write_bytes(payload)

    monkeypatch.setattr(hls_module, "merge_segments", fake_merge)


def test_full_run_saves_sidecar_subtitles(tmp_path, monkeypatch):
    master_url = "https://example.test/master.m3u8"
    responses = {
        master_url: MASTER_PLAYLIST,
        "https://example.test/video.m3u8": VIDEO_PLAYLIST,
        "https://example.test/subs_zh.m3u8": SUBS_PLAYLIST,
        "https://example.test/zh0.vtt": (
            "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:0\n\n"
            "00:00.000 --> 00:02.000\n第一句\n"
        ),
        "https://example.test/zh1.vtt": (
            "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:0\n\n"
            "00:04.000 --> 00:06.000\n第二句\n"
        ),
    }

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target in responses:
            return httpx.Response(200, text=responses[target])
        return httpx.Response(200, content=b"video-bytes")

    monkeypatch.setattr(
        hls_module, "_create_hls_client",
        lambda concurrency: httpx.AsyncClient(transport=httpx.MockTransport(handler)),
    )
    _install_fake_merge(monkeypatch)

    async def run():
        task = _task(tmp_path, master_url)
        await HLSDownloader(task).run()
        assert task.status is TaskStatus.DONE
        output = Path(task.output_path)
        assert output.exists()
        vtt_path = output.with_suffix("").with_name(
            output.with_suffix("").name + ".zh.vtt"
        )
        srt_path = vtt_path.with_suffix(".srt")
        assert vtt_path.exists() and srt_path.exists()
        vtt_text = vtt_path.read_text(encoding="utf-8")
        assert "第一句" in vtt_text and "第二句" in vtt_text
        srt_text = srt_path.read_text(encoding="utf-8")
        assert "00:00:00,000 --> 00:00:02,000" in srt_text

    asyncio.run(run())


def test_subtitle_failure_never_fails_a_verified_download(tmp_path, monkeypatch):
    master_url = "https://example.test/master.m3u8"
    responses = {
        master_url: MASTER_PLAYLIST,
        "https://example.test/video.m3u8": VIDEO_PLAYLIST,
    }

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target in responses:
            return httpx.Response(200, text=responses[target])
        if target.endswith("subs_zh.m3u8"):
            return httpx.Response(404, request=request)
        return httpx.Response(200, content=b"video-bytes")

    monkeypatch.setattr(
        hls_module, "_create_hls_client",
        lambda concurrency: httpx.AsyncClient(transport=httpx.MockTransport(handler)),
    )
    _install_fake_merge(monkeypatch)

    async def run():
        task = _task(tmp_path, master_url)
        await HLSDownloader(task).run()
        assert task.status is TaskStatus.DONE
        output = Path(task.output_path)
        assert output.exists()
        assert not list(output.parent.glob("*.vtt"))

    asyncio.run(run())


def test_subtitles_can_be_disabled_in_settings(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "download_subtitles", False, raising=False)
    master_url = "https://example.test/master.m3u8"
    responses = {
        master_url: MASTER_PLAYLIST,
        "https://example.test/video.m3u8": VIDEO_PLAYLIST,
    }
    subtitle_requests = {"count": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target in responses:
            return httpx.Response(200, text=responses[target])
        if "subs" in target or target.endswith(".vtt"):
            subtitle_requests["count"] += 1
            return httpx.Response(200, text="WEBVTT\n")
        return httpx.Response(200, content=b"video-bytes")

    monkeypatch.setattr(
        hls_module, "_create_hls_client",
        lambda concurrency: httpx.AsyncClient(transport=httpx.MockTransport(handler)),
    )
    _install_fake_merge(monkeypatch)

    async def run():
        task = _task(tmp_path, master_url)
        await HLSDownloader(task).run()
        assert task.status is TaskStatus.DONE
        assert subtitle_requests["count"] == 0
        assert not list(Path(task.output_path).parent.glob("*.vtt"))

    asyncio.run(run())
