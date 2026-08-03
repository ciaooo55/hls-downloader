import asyncio
import json
from pathlib import Path

import httpx
import pytest

from backend.app.config import settings
from backend.app.downloader import dash_native as native_module
from backend.app.downloader.dash_native import NativeDashEngine
from backend.app.models import Task, TaskStatus


MPD_NS = 'xmlns="urn:mpeg:dash:schema:mpd:2011"'


def _live_mpd(rows: str, *, dynamic: bool = True, audio_rows: str = "") -> str:
    mpd_type = "dynamic" if dynamic else "static"
    extra = "" if dynamic else ' mediaPresentationDuration="PT1M"'
    audio = ""
    if audio_rows:
        audio = f"""
    <AdaptationSet mimeType="audio/mp4" lang="zh">
      <Representation id="a1" bandwidth="128000">
        <SegmentTemplate media="a-$Time$.m4s" initialization="a-init.mp4" timescale="1">
          <SegmentTimeline>{audio_rows}</SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>"""
    return f"""<MPD {MPD_NS} type="{mpd_type}"{extra} minimumUpdatePeriod="PT1S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v1" bandwidth="1000000" width="1280" height="720">
        <SegmentTemplate media="v-$Time$.m4s" initialization="v-init.mp4" timescale="1">
          <SegmentTimeline>{rows}</SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>{audio}
  </Period>
</MPD>"""


def _task(tmp_path: Path) -> Task:
    task = Task(id="dash-live", url="https://cdn.test/live/main.mpd", filename="直播")
    task.cancel_event = asyncio.Event()
    task.pause_event = asyncio.Event()
    task.engine_state["temp_dir"] = str(tmp_path / "temp")
    task.engine_state["output_dir"] = str(tmp_path / "out")
    return task


def _install(monkeypatch, handler):
    real_client = httpx.AsyncClient

    def fake_client(**kwargs):
        return real_client(
            transport=httpx.MockTransport(handler), follow_redirects=True
        )

    monkeypatch.setattr(native_module.httpx, "AsyncClient", fake_client)
    monkeypatch.setattr(native_module, "LIVE_MIN_POLL_SECONDS", 0.05)

    async def fake_ffmpeg(command, task=None, duration_sec=0, on_progress=None):
        inputs = [
            command[i + 1] for i, value in enumerate(command[:-1]) if value == "-i"
        ]
        Path(command[-1]).write_bytes(
            b"".join(Path(p).read_bytes() for p in inputs)
        )
        return True

    async def fake_verify(ffmpeg_path, output_path, expected_duration):
        return None

    monkeypatch.setattr(native_module, "_run_ffmpeg", fake_ffmpeg)
    monkeypatch.setattr(native_module, "_verify_output", fake_verify)


def test_records_dynamic_timeline_until_it_turns_static(tmp_path, monkeypatch):
    polls = {"count": 0}
    media: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            polls["count"] += 1
            if polls["count"] == 1:
                return httpx.Response(200, text=_live_mpd(
                    '<S t="0" d="2"/><S d="2"/>',
                    audio_rows='<S t="0" d="2"/><S d="2"/>',
                ))
            return httpx.Response(200, text=_live_mpd(
                '<S t="0" d="2"/><S d="2"/><S d="2"/>',
                dynamic=False,
                audio_rows='<S t="0" d="2"/><S d="2"/><S d="2"/>',
            ))
        media.append(target)
        return httpx.Response(200, content=target.rsplit("/", 1)[-1].encode())

    _install(monkeypatch, handler)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert task.engine_state.get("live") is True
        payload = Path(task.output_path).read_bytes()
        # Video track (init + 3 segments) then audio track, all exactly once.
        assert payload == (
            b"v-init.mp4v-0.m4sv-2.m4sv-4.m4s"
            b"a-init.mp4a-0.m4sa-2.m4sa-4.m4s"
        )
        assert media.count("https://cdn.test/live/v-2.m4s") == 1
        assert "已结束" in task.last_log or "完成" in task.last_log

    asyncio.run(run())


def test_live_representation_change_finalizes_before_mixing_new_track(
    tmp_path, monkeypatch
):
    polls = {"count": 0}
    media: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            polls["count"] += 1
            manifest = _live_mpd('<S t="0" d="2"/><S d="2"/>')
            if polls["count"] > 1:
                manifest = manifest.replace('id="v1"', 'id="v2"')
            return httpx.Response(200, text=manifest)
        media.append(target)
        return httpx.Response(200, content=target.rsplit("/", 1)[-1].encode())

    _install(monkeypatch, handler)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert polls["count"] >= 2
        assert Path(task.output_path).read_bytes() == (
            b"v-init.mp4v-0.m4sv-2.m4s"
        )
        assert len(media) == 3

    asyncio.run(run())


def test_live_manifest_503_uses_shared_cooldown_and_recovers(tmp_path, monkeypatch):
    polls = {"count": 0}
    cooldowns: list[float] = []

    class RecordingWindow(native_module.SharedRetryWindow):
        async def extend(self, delay):
            cooldowns.append(delay)
            return await super().extend(delay)

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            polls["count"] += 1
            if polls["count"] == 1:
                return httpx.Response(200, text=_live_mpd('<S t="0" d="2"/>'))
            if polls["count"] == 2:
                return httpx.Response(503, request=request)
            return httpx.Response(
                200,
                text=_live_mpd('<S t="0" d="2"/><S d="2"/>', dynamic=False),
            )
        return httpx.Response(200, content=b"segment")

    _install(monkeypatch, handler)
    monkeypatch.setattr(native_module, "SharedRetryWindow", RecordingWindow)
    monkeypatch.setattr(native_module, "retry_delay_seconds", lambda *_args: 0.01)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert polls["count"] == 3
        assert cooldowns == [0.01]
        assert task.progress.reconnect_count >= 1

    asyncio.run(run())


def test_live_segment_failure_retries_same_timeline_identity_on_next_manifest(
    tmp_path, monkeypatch
):
    """A transient signed-segment failure must not advance the live cursor."""
    polls = {"manifest": 0, "segment": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            polls["manifest"] += 1
            # Keep the same timeline identity visible until the second poll;
            # then end the event so the successful retry is finalized.
            return httpx.Response(
                200,
                text=_live_mpd(
                    '<S t="0" d="2"/>',
                    dynamic=polls["manifest"] < 3,
                ),
            )
        if target.endswith("v-0.m4s"):
            polls["segment"] += 1
            if polls["segment"] == 1:
                return httpx.Response(503, request=request)
        return httpx.Response(200, content=target.rsplit("/", 1)[-1].encode())

    _install(monkeypatch, handler)
    monkeypatch.setattr(native_module, "MAX_RETRIES", 1)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert polls["manifest"] >= 3
        assert polls["segment"] == 2
        assert Path(task.output_path).read_bytes() == b"v-init.mp4v-0.m4s"

    asyncio.run(run())


def test_stop_request_finalizes_partial_recording(tmp_path, monkeypatch):
    holder: dict[str, Task] = {}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            return httpx.Response(200, text=_live_mpd('<S t="0" d="2"/><S d="2"/>'))
        if target.endswith("v-2.m4s") and "task" in holder:
            holder.pop("task").pause_event.set()
        return httpx.Response(200, content=b"seg")

    _install(monkeypatch, handler)

    async def run():
        task = _task(tmp_path)
        holder["task"] = task
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert Path(task.output_path).exists()
        # The segment interrupted mid-body by the stop request is dropped;
        # only fully received segments are merged.
        assert task.progress.media_duration == pytest.approx(2.0)

    asyncio.run(run())


def test_resume_reuses_recorded_segments(tmp_path, monkeypatch):
    media: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            return httpx.Response(200, text=_live_mpd(
                '<S t="0" d="2"/><S d="2"/><S d="2"/>', dynamic=False,
            ))
        media.append(target)
        return httpx.Response(200, content=b"new")

    _install(monkeypatch, handler)

    async def run():
        task = _task(tmp_path)
        task.engine_state["live"] = True
        downloader = NativeDashEngine(task)
        from backend.app.downloader.engine import task_work_dir

        task_dir = task_work_dir(task)
        seg_dir = task_dir / "segments"
        seg_dir.mkdir(parents=True, exist_ok=True)
        (seg_dir / "000000.seg").write_bytes(b"old0")
        (seg_dir / "000001.seg").write_bytes(b"old1")
        (task_dir / "maps").mkdir(parents=True, exist_ok=True)
        (task_dir / "maps" / "dash-video.init").write_bytes(b"init")
        (task_dir / "live_state.json").write_text(json.dumps({
            "version": 1,
            "tracks": {"video": {"segments": [
                {"index": 0, "identity": 0, "duration": 2.0},
                {"index": 1, "identity": 2, "duration": 2.0},
            ]}},
        }), encoding="utf-8")
        handled = await downloader.run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        # Only the genuinely new segment (t=4) was fetched.
        assert [url for url in media if url.endswith(".m4s")] == [
            "https://cdn.test/live/v-4.m4s"
        ]
        payload = Path(task.output_path).read_bytes()
        assert payload == b"initold0old1new"

    asyncio.run(run())


def test_stall_finalizes_with_recorded_content(tmp_path, monkeypatch):
    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            return httpx.Response(200, text=_live_mpd('<S t="0" d="2"/>'))
        return httpx.Response(200, content=b"seg")

    _install(monkeypatch, handler)
    monkeypatch.setattr(native_module, "LIVE_STALL_MIN_SECONDS", 0.3)
    monkeypatch.setattr(native_module, "LIVE_STALL_TARGET_MULTIPLIER", 0.01)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert "停止更新" in (task.last_log or "") or Path(task.output_path).exists()

    asyncio.run(run())


def test_duration_limit_stops_the_recording(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "live_record_max_minutes", 1, raising=False)

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            return httpx.Response(200, text=_live_mpd('<S t="0" d="40"/><S d="40"/>'))
        return httpx.Response(200, content=b"x")

    _install(monkeypatch, handler)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert task.progress.media_duration == pytest.approx(80.0)
        assert Path(task.output_path).exists()

    asyncio.run(run())


def test_offline_manifest_finalizes_recorded_segments(tmp_path, monkeypatch):
    """A finished live event whose MPD 404s must still produce the file."""
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(404, request=request)

    _install(monkeypatch, handler)

    async def run():
        task = _task(tmp_path)
        task.engine_state["live"] = True
        engine = NativeDashEngine(task)
        from backend.app.downloader.engine import task_work_dir

        task_dir = task_work_dir(task)
        seg_dir = task_dir / "segments"
        seg_dir.mkdir(parents=True, exist_ok=True)
        (seg_dir / "000000.seg").write_bytes(b"one")
        (seg_dir / "000001.seg").write_bytes(b"two")
        (task_dir / "maps").mkdir(parents=True, exist_ok=True)
        (task_dir / "maps" / "dash-video.init").write_bytes(b"init")
        (task_dir / "manifest.mpd").write_text(
            _live_mpd('<S t="0" d="2"/><S d="2"/>'), encoding="utf-8"
        )
        (task_dir / "live_state.json").write_text(json.dumps({
            "version": 1,
            "tracks": {"video": {"segments": [
                {"index": 0, "identity": 0, "duration": 2.0, "start": 0.0},
                {"index": 1, "identity": 2, "duration": 2.0, "start": 2.0},
            ]}},
        }), encoding="utf-8")

        handled = await engine.run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        assert Path(task.output_path).read_bytes() == b"initonetwo"
        assert task.progress.media_duration == pytest.approx(4.0)

    asyncio.run(run())


def test_recorded_segments_are_never_reused_as_vod_positions(tmp_path, monkeypatch):
    """After the stream turns static, resume keeps recording semantics."""
    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            # The event is over: same URL now serves the whole VOD timeline.
            return httpx.Response(200, text=_live_mpd(
                '<S t="0" d="2"/><S d="2"/><S d="2"/><S d="2"/>', dynamic=False,
            ))
        return httpx.Response(200, content=b"fresh")

    _install(monkeypatch, handler)

    async def run():
        task = _task(tmp_path)
        task.engine_state["live"] = True
        from backend.app.downloader.engine import task_work_dir

        task_dir = task_work_dir(task)
        seg_dir = task_dir / "segments"
        seg_dir.mkdir(parents=True, exist_ok=True)
        # Recording joined the live window at t=4s, so file 0 is NOT VOD 0.
        (seg_dir / "000000.seg").write_bytes(b"rec-t4")
        (task_dir / "maps").mkdir(parents=True, exist_ok=True)
        (task_dir / "maps" / "dash-video.init").write_bytes(b"init")
        (task_dir / "live_state.json").write_text(json.dumps({
            "version": 1,
            "tracks": {"video": {"segments": [
                {"index": 0, "identity": 4, "duration": 2.0, "start": 4.0},
            ]}},
        }), encoding="utf-8")

        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        payload = Path(task.output_path).read_bytes()
        # The recorded segment stays first and only newer identities follow;
        # the static manifest's earlier segments are never spliced under it.
        assert payload == b"initrec-t4fresh"

    asyncio.run(run())


def test_track_start_offsets_reach_the_mux_command(tmp_path, monkeypatch):
    """Live tracks starting at different times must be offset, not stacked."""
    commands: list[list[str]] = []

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target.endswith("main.mpd"):
            # Video window starts at t=0, audio window starts at t=2.
            return httpx.Response(200, text=_live_mpd(
                '<S t="0" d="2"/><S d="2"/>', dynamic=False,
                audio_rows='<S t="2" d="2"/>',
            ))
        return httpx.Response(200, content=b"x")

    _install(monkeypatch, handler)

    async def capture(command, task=None, duration_sec=0, on_progress=None):
        commands.append(list(command))
        Path(command[-1]).write_bytes(b"muxed")
        return True

    monkeypatch.setattr(native_module, "_run_ffmpeg", capture)

    async def run():
        task = _task(tmp_path)
        handled = await NativeDashEngine(task).run()
        assert handled is True
        assert task.status is TaskStatus.DONE
        command = commands[-1]
        assert "-itsoffset" in command
        offset = command[command.index("-itsoffset") + 1]
        assert float(offset) == pytest.approx(2.0, abs=0.01)
        # The offset must immediately precede its own input.
        assert command[command.index("-itsoffset") + 2] == "-i"

    asyncio.run(run())


def test_dash_live_checkpoint_journal_replays_incremental_tracks(tmp_path):
    task = _task(tmp_path)
    engine = NativeDashEngine(task)
    from backend.app.downloader.engine import task_work_dir

    task_dir = task_work_dir(task)
    metadata = {
        "fingerprint": "track-one", "mime": "video/mp4", "codecs": "avc1",
        "has_init": True, "init_size": 4,
    }
    first = {"index": 0, "identity": 0, "duration": 2.0, "start": 0.0, "size": 3}
    second = {"index": 1, "identity": 2, "duration": 2.0, "start": 2.0, "size": 3}
    state = {"video": {"entries": [first], **metadata}}
    engine._save_live_state(task_dir, state)
    state["video"]["entries"].append(second)
    engine._save_live_state(
        task_dir,
        state,
        changed_tracks={"video": [second]},
    )
    journal = task_dir / "live_state.journal"
    assert journal.exists()
    event = json.loads(journal.read_text(encoding="utf-8").splitlines()[0])
    assert [item["identity"] for item in event["tracks"]["video"]["upsert"]] == [2]

    restarted_engine = NativeDashEngine(task)
    restored = restarted_engine._load_live_state(task_dir)
    assert restored is not None
    assert [item["identity"] for item in restored["tracks"]["video"]["segments"]] == [0, 2]
    engine._save_live_state(
        task_dir,
        state,
        force_compact=True,
    )
    assert not journal.exists()


def test_dash_live_interleaved_tracks_restore_before_torn_tail(tmp_path):
    task = _task(tmp_path)
    engine = NativeDashEngine(task)
    from backend.app.downloader.engine import task_work_dir

    task_dir = task_work_dir(task)
    video_meta = {
        "fingerprint": "video-one", "mime": "video/mp4", "codecs": "avc1",
        "has_init": True, "init_size": 4,
    }
    audio_meta = {
        "fingerprint": "audio-one", "mime": "audio/mp4", "codecs": "mp4a",
        "has_init": True, "init_size": 4,
    }
    video0 = {"index": 0, "identity": 0, "duration": 2.0, "start": 0.0, "size": 3}
    video1 = {"index": 1, "identity": 2, "duration": 2.0, "start": 2.0, "size": 3}
    audio0 = {"index": 0, "identity": 0, "duration": 2.0, "start": 0.0, "size": 2}
    audio1 = {"index": 1, "identity": 2, "duration": 2.0, "start": 2.0, "size": 2}
    state = {
        "video": {"entries": [video0], **video_meta},
        "audio": {"entries": [audio0], **audio_meta},
    }
    engine._save_live_state(task_dir, state)
    state["video"]["entries"].append(video1)
    engine._save_live_state(task_dir, state, changed_tracks={"video": [video1]})
    state["audio"]["entries"].append(audio1)
    engine._save_live_state(task_dir, state, changed_tracks={"audio": [audio1]})
    journal = task_dir / "live_state.journal"
    with journal.open("a", encoding="utf-8") as stream:
        stream.write('{"version":1,"tracks":')

    restarted_engine = NativeDashEngine(task)
    restored = restarted_engine._load_live_state(task_dir)
    assert restored is not None
    assert [item["identity"] for item in restored["tracks"]["video"]["segments"]] == [0, 2]
    assert [item["identity"] for item in restored["tracks"]["audio"]["segments"]] == [0, 2]

    # A full checkpoint may remove a disappeared track; the deletion must also
    # survive journal replay rather than resurrecting stale audio on restart.
    restarted_engine._save_live_state(task_dir, {"video": state["video"]})
    without_audio = NativeDashEngine(task)._load_live_state(task_dir)
    assert without_audio is not None
    assert set(without_audio["tracks"]) == {"video"}
