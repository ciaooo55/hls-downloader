import asyncio
import json
from pathlib import Path

import httpx
import pytest

from backend.app.downloader import dash_native as native_module
from backend.app.downloader.dash import DashDownloader
from backend.app.downloader.dash_native import NativeDashEngine, _merge_segmented_ttml
from backend.app.models import Task, TaskStatus


MPD_NS = 'xmlns="urn:mpeg:dash:schema:mpd:2011"'
TEMPLATE_MPD = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT8S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4"
        duration="4" timescale="1" startNumber="1"/>
      <Representation id="v720" bandwidth="2000000" width="1280" height="720"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="zh">
      <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4"
        duration="4" timescale="1"/>
      <Representation id="a1" bandwidth="128000"/>
    </AdaptationSet>
  </Period>
</MPD>"""


def _task(tmp_path: Path, url: str = "https://cdn.test/stream/manifest.mpd") -> Task:
    task = Task(id="dash-test", url=url, filename="影片")
    task.cancel_event = asyncio.Event()
    task.pause_event = asyncio.Event()
    task.engine_state["temp_dir"] = str(tmp_path / "temp")
    task.engine_state["output_dir"] = str(tmp_path / "out")
    return task


def _install_transport(monkeypatch, handler):
    real_client = httpx.AsyncClient

    def fake_client(**kwargs):
        return real_client(
            transport=httpx.MockTransport(handler), follow_redirects=True
        )

    monkeypatch.setattr(native_module.httpx, "AsyncClient", fake_client)


def _install_fake_mux(monkeypatch):
    async def fake_ffmpeg(command, task=None, duration_sec=0, on_progress=None):
        inputs = [
            command[index + 1]
            for index, value in enumerate(command[:-1])
            if value == "-i"
        ]
        payload = b"".join(Path(value).read_bytes() for value in inputs)
        Path(command[-1]).write_bytes(payload)
        return True

    async def fake_verify(ffmpeg_path, output_path, expected_duration):
        return None

    monkeypatch.setattr(native_module, "_run_ffmpeg", fake_ffmpeg)
    monkeypatch.setattr(native_module, "_verify_output", fake_verify)


def test_native_dash_downloads_and_muxes_both_tracks(tmp_path, monkeypatch):
    requests: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        requests.append(target)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=TEMPLATE_MPD)
        name = target.rsplit("/", 2)[-2] + "/" + target.rsplit("/", 1)[-1]
        return httpx.Response(200, content=name.encode())

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        output = Path(task.output_path)
        assert output.suffix == ".mp4"
        payload = output.read_bytes()
        # Video track first (init + 2 segments), then the audio track.
        assert payload == b"v/init.mp4v/1.m4sv/2.m4sa/init.mp4a/1.m4sa/2.m4s"
        assert task.progress.total_segments == 6
        assert task.progress.completed_segments == 6
        assert task.progress.progress_percent == pytest.approx(100.0)

    asyncio.run(run())


def test_native_dash_pause_keeps_segments_and_resume_reuses_them(tmp_path, monkeypatch):
    media_hits: dict[str, int] = {}
    pause_target: dict[str, Task] = {}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=TEMPLATE_MPD)
        media_hits[target] = media_hits.get(target, 0) + 1
        if target.endswith("v/2.m4s") and "task" in pause_target:
            pause_target["task"].pause_event.set()
            pause_target.pop("task")
        return httpx.Response(200, content=b"x" * 16)

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        pause_target["task"] = task
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.PAUSED
        assert not task.pause_event.is_set()

        first_round = dict(media_hits)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        # Segments finished before the pause are reused, not re-downloaded.
        # v/2.m4s itself was interrupted mid-body, so it is fetched again.
        for url, count in first_round.items():
            if url.endswith("v/2.m4s"):
                assert media_hits[url] == count + 1
                continue
            if url.endswith((".m4s", "init.mp4")):
                assert media_hits[url] == count, url

    asyncio.run(run())


def test_native_dash_resume_ignores_rotated_signature_but_rejects_new_representation(
    tmp_path, monkeypatch
):
    monkeypatch.setattr(native_module.settings, "keep_temp_files", True)
    active_manifest = {"text": TEMPLATE_MPD}
    media_hits: dict[str, int] = {}

    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path.endswith("manifest.mpd"):
            return httpx.Response(200, text=active_manifest["text"], request=request)
        stable = request.url.path
        media_hits[stable] = media_hits.get(stable, 0) + 1
        return httpx.Response(200, content=stable.encode(), request=request)

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path, "https://cdn.test/stream/manifest.mpd?token=old-secret")
        assert await NativeDashEngine(task).run() is True
        first_hits = dict(media_hits)
        state_path = (
            tmp_path / "temp" / ".tasks" / task.id / "dash_vod_segments.json"
        )
        checkpoint = state_path.read_text(encoding="utf-8")
        assert "old-secret" not in checkpoint
        assert "https://" not in checkpoint

        task.url = "https://cdn.test/stream/manifest.mpd?token=new-secret"
        assert await NativeDashEngine(task).run() is True
        assert media_hits == first_hits

        active_manifest["text"] = TEMPLATE_MPD.replace('id="v720"', 'id="v1080"')
        assert await NativeDashEngine(task).run() is True
        assert media_hits["/stream/v/init.mp4"] == first_hits["/stream/v/init.mp4"] + 1
        assert media_hits["/stream/v/1.m4s"] == first_hits["/stream/v/1.m4s"] + 1
        assert media_hits["/stream/v/2.m4s"] == first_hits["/stream/v/2.m4s"] + 1
        assert media_hits["/stream/a/init.mp4"] == first_hits["/stream/a/init.mp4"]

    asyncio.run(run())


def test_facade_falls_back_to_ytdlp_for_dynamic_mpd(tmp_path, monkeypatch):
    dynamic = f'<MPD {MPD_NS} type="dynamic"><Period/></MPD>'

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, text=dynamic)

    _install_transport(monkeypatch, handler)
    fallback_called = {"count": 0}

    def fake_ytdlp(self, task_dir: Path) -> str:
        fallback_called["count"] += 1
        payload = task_dir / "payload.mp4"
        payload.write_bytes(b"ytdlp-output")
        return str(payload)

    monkeypatch.setattr(DashDownloader, "_run_ytdlp", fake_ytdlp)

    async def run():
        task = _task(tmp_path)
        await DashDownloader(task).run()
        assert fallback_called["count"] == 1
        assert task.status is TaskStatus.DONE
        assert Path(task.output_path).read_bytes() == b"ytdlp-output"

    asyncio.run(run())


def test_facade_marks_drm_mpd_unsupported_without_fallback(tmp_path, monkeypatch):
    drm = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT4S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"/>
      <SegmentTemplate media="$Number$.m4s" duration="4" timescale="1"/>
      <Representation id="v" bandwidth="1" width="2" height="2"/>
    </AdaptationSet>
  </Period>
</MPD>"""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, text=drm)

    _install_transport(monkeypatch, handler)

    def fail_ytdlp(self, task_dir: Path) -> str:
        raise AssertionError("DRM content must not reach the fallback engine")

    monkeypatch.setattr(DashDownloader, "_run_ytdlp", fail_ytdlp)

    async def run():
        task = _task(tmp_path)
        await DashDownloader(task).run()
        assert task.status is TaskStatus.UNSUPPORTED
        assert "DRM" in (task.error_message or "").upper()

    asyncio.run(run())


def test_cancel_mid_download_raises_and_marks_canceled(tmp_path, monkeypatch):
    cancel_target: dict[str, Task] = {}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=TEMPLATE_MPD)
        if target.endswith("v/2.m4s") and "task" in cancel_target:
            cancel_target.pop("task").cancel_event.set()
        return httpx.Response(200, content=b"x" * 16)

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        cancel_target["task"] = task
        with pytest.raises(asyncio.CancelledError):
            await DashDownloader(task).run()
        assert task.status is TaskStatus.CANCELED

    asyncio.run(run())


def test_retry_after_failure_completes_on_second_run(tmp_path, monkeypatch):
    fail_once = {"active": True}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=TEMPLATE_MPD)
        if target.endswith("v/2.m4s") and fail_once["active"]:
            return httpx.Response(404, request=request)
        return httpx.Response(200, content=b"z" * 8)

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        await DashDownloader(task).run()
        assert task.status is TaskStatus.FAILED

        fail_once["active"] = False
        task.error_message = ""
        task.error_code = ""
        await DashDownloader(task).run()
        assert task.status is TaskStatus.DONE
        assert Path(task.output_path).exists()

    asyncio.run(run())


def test_audio_only_mpd_muxes_single_track(tmp_path, monkeypatch):
    audio_only = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT8S">
  <Period>
    <AdaptationSet mimeType="audio/mp4" lang="zh">
      <SegmentTemplate media="a/$Number$.m4s" initialization="a/init.mp4"
        duration="4" timescale="1"/>
      <Representation id="a1" bandwidth="128000"/>
    </AdaptationSet>
  </Period>
</MPD>"""

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=audio_only)
        name = target.rsplit("/", 2)[-2] + "/" + target.rsplit("/", 1)[-1]
        return httpx.Response(200, content=name.encode())

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        payload = Path(task.output_path).read_bytes()
        assert payload == b"a/init.mp4a/1.m4sa/2.m4s"
        assert task.progress.total_segments == 3

    asyncio.run(run())


def test_single_file_representation_downloads_through_engine(tmp_path, monkeypatch):
    single = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT6S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v" bandwidth="900000" width="640" height="360">
        <BaseURL>full/video-file.mp4</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"""

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=single)
        assert target.endswith("full/video-file.mp4")
        return httpx.Response(200, content=b"whole-file-bytes")

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert Path(task.output_path).read_bytes() == b"whole-file-bytes"

    asyncio.run(run())


def test_facade_falls_back_when_response_is_not_mpd(tmp_path, monkeypatch):
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, text="#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nv.m3u8\n")

    _install_transport(monkeypatch, handler)
    fallback = {"count": 0}

    def fake_ytdlp(self, task_dir: Path) -> str:
        fallback["count"] += 1
        payload = task_dir / "payload.mp4"
        payload.write_bytes(b"fallback-bytes")
        return str(payload)

    monkeypatch.setattr(DashDownloader, "_run_ytdlp", fake_ytdlp)

    async def run():
        task = _task(tmp_path)
        await DashDownloader(task).run()
        assert fallback["count"] == 1
        assert task.status is TaskStatus.DONE

    asyncio.run(run())


def test_video_track_is_previewable_while_downloading(tmp_path, monkeypatch):
    from backend.app.config import settings as app_settings

    monkeypatch.setattr(app_settings, "keep_temp_files", True, raising=False)
    # The playback service resolves task dirs from global settings.
    monkeypatch.setattr(app_settings, "download_dir", str(tmp_path / "temp"), raising=False)
    monkeypatch.setattr(app_settings, "temp_dir", str(tmp_path / "temp"), raising=False)

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=TEMPLATE_MPD)
        name = target.rsplit("/", 2)[-2] + "/" + target.rsplit("/", 1)[-1]
        return httpx.Response(200, content=name.encode())

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        task_dir = tmp_path / "temp" / ".tasks" / task.id
        # The video track lands in the playback service layout with a full
        # plan, so preview/casting work while the download is running.
        plan = json.loads(
            (task_dir / "playback-plan.json").read_text(encoding="utf-8")
        )
        assert [entry["index"] for entry in plan["segments"]] == [0, 1]
        assert all(
            entry["init_name"] == "dash-video.init" for entry in plan["segments"]
        )
        assert (task_dir / "segments" / "000000.seg").read_bytes() == b"v/1.m4s"
        assert (task_dir / "maps" / "dash-video.init").read_bytes() == b"v/init.mp4"
        assert task.progress.playable_segments == 2
        assert task.progress.playable_duration == pytest.approx(8.0)

    asyncio.run(run())


def test_reserve_output_forces_muxable_container(tmp_path):
    cases = {
        "Some.Show.S01E02.1080p": "Some.Show.S01E02.1080p.mp4",
        "movie.mpd": "movie.mp4",
        "clip": "clip.mp4",
        "已有后缀.mp4": "已有后缀.mp4",
    }
    for raw, expected in cases.items():
        task = _task(tmp_path)
        task.filename = raw
        reserved = NativeDashEngine._reserve_output(task)
        assert reserved.name == expected, raw
        # The name is claimed atomically: the reservation exists on disk and
        # a second same-named reservation gets a distinct file.
        assert reserved.exists()
        duplicate = NativeDashEngine._reserve_output(task)
        assert duplicate.name != reserved.name
        reserved.unlink()
        duplicate.unlink()


def test_webm_tracks_mux_into_mkv_container(tmp_path, monkeypatch):
    webm = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT8S">
  <Period>
    <AdaptationSet mimeType="video/webm">
      <SegmentTemplate media="v/$Number$.webm" duration="4" timescale="1"/>
      <Representation id="v" bandwidth="900000" width="640" height="360" codecs="vp8"/>
    </AdaptationSet>
  </Period>
</MPD>"""

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=webm)
        return httpx.Response(200, content=b"w" * 8)

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert Path(task.output_path).suffix == ".mkv"

    asyncio.run(run())


def test_mux_failure_leaves_no_file_in_download_directory(tmp_path, monkeypatch):
    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=TEMPLATE_MPD)
        return httpx.Response(200, content=b"x" * 8)

    _install_transport(monkeypatch, handler)

    async def failing_ffmpeg(command, task=None, duration_sec=0, on_progress=None):
        return False

    monkeypatch.setattr(native_module, "_run_ffmpeg", failing_ffmpeg)

    async def run():
        task = _task(tmp_path)
        await DashDownloader(task).run()
        assert task.status is TaskStatus.FAILED
        out_dir = tmp_path / "out"
        leftovers = list(out_dir.glob("*")) if out_dir.exists() else []
        # Neither the zero-byte reservation nor a .merging temp may remain.
        assert leftovers == []

    asyncio.run(run())


def test_concurrent_workers_share_rate_limit_window(tmp_path, monkeypatch):
    extend_calls: list[float] = []

    class RecordingWindow(native_module.SharedRetryWindow):
        async def extend(self, delay):
            extend_calls.append(delay)
            return await super().extend(delay)

    monkeypatch.setattr(native_module, "SharedRetryWindow", RecordingWindow)
    hits: dict[str, int] = {}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=TEMPLATE_MPD)
        hits[target] = hits.get(target, 0) + 1
        if target.endswith("a/1.m4s") and hits[target] == 1:
            return httpx.Response(
                429, headers={"Retry-After": "0.2"}, request=request
            )
        return httpx.Response(200, content=b"r" * 8)

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        task.concurrency = 4
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert task.progress.max_workers == 4
        assert task.progress.completed_segments == 6
        retried = [url for url, count in hits.items() if url.endswith("a/1.m4s")]
        assert retried and hits[retried[0]] == 2
        assert len(extend_calls) >= 1
        assert extend_calls[0] >= 0.2

    asyncio.run(run())


def test_pause_under_concurrency_leaves_only_complete_segments(tmp_path, monkeypatch):
    pause_target: dict[str, Task] = {}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=TEMPLATE_MPD)
        if target.endswith("v/1.m4s") and "task" in pause_target:
            pause_target.pop("task").pause_event.set()
        return httpx.Response(200, content=b"p" * 8)

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        task.concurrency = 4
        pause_target["task"] = task
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.PAUSED
        temp_root = tmp_path / "temp"
        assert not list(temp_root.rglob("*.tmp"))
        for segment in temp_root.rglob("*.m4s"):
            assert segment.stat().st_size > 0, segment

    asyncio.run(run())


def test_hard_segment_failure_fails_the_task_with_diagnosis(tmp_path, monkeypatch):
    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("manifest.mpd"):
            return httpx.Response(200, text=TEMPLATE_MPD)
        if target.endswith("v/2.m4s"):
            return httpx.Response(404, request=request)
        return httpx.Response(200, content=b"y" * 8)

    _install_transport(monkeypatch, handler)
    _install_fake_mux(monkeypatch)

    async def run():
        task = _task(tmp_path)
        await DashDownloader(task).run()
        assert task.status is TaskStatus.FAILED
        assert task.error_code

    asyncio.run(run())


def test_segmented_ttml_is_merged_and_relative_times_are_shifted(tmp_path):
    first = tmp_path / "first.ttml"
    second = tmp_path / "second.ttml"
    output = tmp_path / "merged.ttml"
    template = (
        '<tt xmlns="http://www.w3.org/ns/ttml"><body><div>'
        '<p begin="00:00:00.000" end="00:00:01.000">{}</p>'
        '</div></body></tt>'
    )
    first.write_text(template.format("one"), encoding="utf-8")
    second.write_text(template.format("two"), encoding="utf-8")

    _merge_segmented_ttml(
        [first, second],
        [
            {"start": 0.0, "duration": 2.0},
            {"start": 2.0, "duration": 2.0},
        ],
        output,
    )

    text = output.read_text(encoding="utf-8")
    assert "one" in text and "two" in text
    assert 'begin="00:00:02.000"' in text
    assert 'end="00:00:03.000"' in text
