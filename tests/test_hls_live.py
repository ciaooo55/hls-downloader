import asyncio
import json
from pathlib import Path

import httpx
import pytest

from backend.app.config import settings
from backend.app.downloader import hls as hls_module
from backend.app.downloader.hls import HLSDownloader
from backend.app.downloader.parser import parse_m3u8
from backend.app.models import Task, TaskStatus


LIVE_HEAD = "#EXTM3U\n#EXT-X-TARGETDURATION:4\n"


def _live_playlist(first_sequence: int, names: list[str], ended: bool = False) -> str:
    lines = [LIVE_HEAD + f"#EXT-X-MEDIA-SEQUENCE:{first_sequence}"]
    for name in names:
        lines.append("#EXTINF:4,")
        lines.append(name)
    if ended:
        lines.append("#EXT-X-ENDLIST")
    return "\n".join(lines) + "\n"


def _task(tmp_path: Path, url: str = "https://example.test/live.m3u8") -> Task:
    task = Task(id="live-test", url=url, filename="记录")
    task.cancel_event = asyncio.Event()
    task.pause_event = asyncio.Event()
    task.engine_state["temp_dir"] = str(tmp_path / "temp")
    task.engine_state["output_dir"] = str(tmp_path / "out")
    return task


def _downloader(task: Task) -> HLSDownloader:
    downloader = HLSDownloader(task)
    downloader._seg_dir().mkdir(parents=True, exist_ok=True)
    return downloader


def _parsed(url: str, content: str) -> dict:
    parsed = parse_m3u8(url, content)
    parsed["final_url"] = url
    return parsed


async def _instant_wait(self, seconds: float) -> None:
    await asyncio.sleep(0)


def test_live_recording_appends_segments_and_finishes_on_endlist(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    url = "https://example.test/live.m3u8"
    polls = {"count": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target == url:
            polls["count"] += 1
            if polls["count"] == 1:
                return httpx.Response(200, text=_live_playlist(0, ["s0.ts", "s1.ts"]))
            return httpx.Response(
                200,
                text=_live_playlist(1, ["s1.ts", "s2.ts", "s3.ts"], ended=True),
            )
        if target.endswith("s2.ts"):
            return httpx.Response(404, request=request)
        return httpx.Response(200, content=b"segment-bytes")

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, _live_playlist(0, ["s0.ts", "s1.ts"]))
            result = await downloader._record_live(client, parsed, {}, None)
        assert result is not None
        segments, total_duration = result
        # s2 is a hard 404: the live window moves on, so it is skipped and the
        # next kept segment carries a discontinuity marker.
        assert [segment["url"].rsplit("/", 1)[-1] for segment in segments] == [
            "s0.ts",
            "s1.ts",
            "s3.ts",
        ]
        # Indexes are compacted after the drop: the playback service requires
        # plan index == list position, so holes must never reach the plan.
        assert [segment["index"] for segment in segments] == [0, 1, 2]
        assert segments[2]["discontinuity"] is True
        assert total_duration == pytest.approx(12.0)
        assert task.progress.failed_segments == 1
        assert task.engine_state.get("live") is None  # set by run(), not here
        state = json.loads(
            (downloader._task_dir() / "live_state.json").read_text(encoding="utf-8")
        )
        assert len(state["segments"]) == 3
        plan = json.loads(
            (downloader._task_dir() / "playback-plan.json").read_text(encoding="utf-8")
        )
        assert [entry["index"] for entry in plan["segments"]] == [0, 1, 2]
        for segment in segments:
            path = downloader._seg_dir() / f"{segment['index']:06d}.seg"
            assert path.read_bytes() == b"segment-bytes"

    asyncio.run(run())


def test_live_recording_stop_request_finalizes_partial_capture(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    url = "https://example.test/live.m3u8"

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)

        def handler(request: httpx.Request) -> httpx.Response:
            if str(request.url) == url:
                return httpx.Response(200, text=_live_playlist(0, ["s0.ts", "s1.ts"]))
            if str(request.url).endswith("s1.ts"):
                # The stop request lands while the batch is still downloading.
                task.pause_event.set()
            return httpx.Response(200, content=b"x" * 64)

        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, _live_playlist(0, ["s0.ts", "s1.ts"]))
            result = await downloader._record_live(client, parsed, {}, None)
        assert result is not None
        segments, total_duration = result
        assert len(segments) >= 1
        assert total_duration == pytest.approx(4.0 * len(segments))

    asyncio.run(run())


def test_live_recording_respects_duration_limit(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    monkeypatch.setattr(settings, "live_record_max_minutes", 1, raising=False)
    url = "https://example.test/live.m3u8"
    playlist = (
        LIVE_HEAD
        + "#EXT-X-MEDIA-SEQUENCE:0\n"
        + "#EXTINF:40,\nlong0.ts\n#EXTINF:40,\nlong1.ts\n"
    )

    def handler(request: httpx.Request) -> httpx.Response:
        if str(request.url) == url:
            return httpx.Response(200, text=playlist)
        return httpx.Response(200, content=b"y" * 16)

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            result = await downloader._record_live(
                client, _parsed(url, playlist), {}, None
            )
        assert result is not None
        segments, total_duration = result
        assert len(segments) == 2
        assert total_duration == pytest.approx(80.0)
        assert "上限" in task.last_log

    asyncio.run(run())


def test_live_recording_finalizes_when_origin_stalls(tmp_path, monkeypatch):
    monkeypatch.setattr(hls_module, "LIVE_STALL_MIN_SECONDS", 0.3)
    monkeypatch.setattr(hls_module, "LIVE_STALL_TARGET_MULTIPLIER", 0.01)

    async def quick_wait(self, seconds: float) -> None:
        await asyncio.sleep(0.4)

    monkeypatch.setattr(HLSDownloader, "_live_wait", quick_wait)
    url = "https://example.test/live.m3u8"
    playlist = _live_playlist(0, ["s0.ts"])

    def handler(request: httpx.Request) -> httpx.Response:
        if str(request.url) == url:
            return httpx.Response(200, text=playlist)
        return httpx.Response(200, content=b"z" * 8)

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            result = await downloader._record_live(
                client, _parsed(url, playlist), {}, None
            )
        assert result is not None
        segments, _total = result
        assert len(segments) == 1
        assert "停止更新" in task.last_log

    asyncio.run(run())


def test_live_recording_resumes_from_saved_state(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    url = "https://example.test/live.m3u8"

    def handler(request: httpx.Request) -> httpx.Response:
        if str(request.url) == url:
            return httpx.Response(
                200, text=_live_playlist(7, ["s7.ts"], ended=True)
            )
        return httpx.Response(200, content=b"new-segment")

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        seg_dir = downloader._seg_dir()
        (seg_dir / "000000.seg").write_bytes(b"old0")
        # Segment 1 was lost mid-crash: only intact files survive the restore.
        saved_state = {
            "version": 1,
            "total_duration": 8.0,
            "segments": [
                {
                    "index": 0,
                    "url": "https://example.test/s0.ts",
                    "duration": 4.0,
                    "media_sequence": 0,
                    "discontinuity": False,
                    "init_path": "",
                },
                {
                    "index": 1,
                    "url": "https://example.test/s1.ts",
                    "duration": 4.0,
                    "media_sequence": 1,
                    "discontinuity": False,
                    "init_path": "",
                },
            ],
        }
        (downloader._task_dir() / "live_state.json").write_text(
            json.dumps(saved_state), encoding="utf-8"
        )
        loaded = downloader._load_live_state()
        assert loaded is not None
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, _live_playlist(7, ["s7.ts"], ended=True))
            result = await downloader._record_live(client, parsed, {}, loaded)
        assert result is not None
        segments, total_duration = result
        assert [segment["index"] for segment in segments] == [0, 1]
        assert segments[0]["url"].endswith("s0.ts")
        # The first segment of the resumed session marks a discontinuity so
        # players do not assume a continuous timeline across the crash gap.
        assert segments[1]["url"].endswith("s7.ts")
        assert segments[1]["discontinuity"] is True
        assert total_duration == pytest.approx(8.0)

    asyncio.run(run())


def test_restore_compacts_indexes_when_leading_segment_lost(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    url = "https://example.test/live.m3u8"

    def handler(request: httpx.Request) -> httpx.Response:
        if str(request.url) == url:
            return httpx.Response(200, text=_live_playlist(7, ["s7.ts"], ended=True))
        return httpx.Response(200, content=b"fresh")

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        seg_dir = downloader._seg_dir()
        # Segment 0's file was lost in the crash; only segment 1 survived.
        (seg_dir / "000001.seg").write_bytes(b"kept")
        state = {
            "version": 1,
            "total_duration": 8.0,
            "segments": [
                {"index": 0, "url": "https://example.test/s0.ts", "duration": 4.0,
                 "media_sequence": 0, "discontinuity": False, "init_path": ""},
                {"index": 1, "url": "https://example.test/s1.ts", "duration": 4.0,
                 "media_sequence": 1, "discontinuity": False, "init_path": ""},
            ],
        }
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, _live_playlist(7, ["s7.ts"], ended=True))
            result = await downloader._record_live(client, parsed, {}, state)
        assert result is not None
        segments, _total = result
        # The survivor is renumbered down to 0 (its file renamed with it), so
        # the playback plan keeps index == position and stays servable.
        assert [segment["index"] for segment in segments] == [0, 1]
        assert segments[0]["url"].endswith("s1.ts")
        assert (seg_dir / "000000.seg").read_bytes() == b"kept"
        assert (seg_dir / "000001.seg").read_bytes() == b"fresh"
        plan = json.loads(
            (downloader._task_dir() / "playback-plan.json").read_text(encoding="utf-8")
        )
        assert [entry["index"] for entry in plan["segments"]] == [0, 1]

    asyncio.run(run())


def test_restore_rejects_nonempty_segment_with_wrong_persisted_size(tmp_path):
    task = _task(tmp_path, "https://example.test/live.m3u8")
    downloader = _downloader(task)
    (downloader._seg_dir() / "000000.seg").write_bytes(b"truncated")
    recorded: list[dict] = []

    duration = downloader._restore_live_segments({
        "version": 2,
        "segments": [{
            "index": 0,
            "url": "https://example.test/s0.ts",
            "duration": 4.0,
            "media_sequence": 0,
            "size": 100,
        }],
    }, recorded)

    assert duration == 0
    assert recorded == []


def test_live_checkpoint_does_not_persist_signed_urls_and_restores_local_map(tmp_path):
    task = _task(tmp_path, "https://example.test/live.m3u8?token=manifest-secret")
    downloader = _downloader(task)
    segment_path = downloader._seg_dir() / "000000.seg"
    segment_path.write_bytes(b"media")
    map_dir = downloader._task_dir() / "maps"
    map_dir.mkdir(parents=True, exist_ok=True)
    init_path = map_dir / "safe.init"
    init_path.write_bytes(b"init")
    recorded = [{
        "index": 0,
        "url": "https://example.test/part.m4s?token=segment-secret",
        "duration": 4.0,
        "media_sequence": 1,
        "discontinuity": False,
        "init_path": str(init_path),
    }]

    downloader._save_live_state(recorded, 4.0)
    raw = (downloader._task_dir() / "live_state.json").read_text(encoding="utf-8")
    state = json.loads(raw)
    restored: list[dict] = []
    duration = downloader._restore_live_segments(state, restored)

    assert "manifest-secret" not in raw
    assert "segment-secret" not in raw
    assert "https://" not in raw
    assert state["version"] == 3
    assert restored[0]["init_path"] == str(init_path.resolve())
    assert duration == pytest.approx(4.0)


def test_live_checkpoint_rejects_init_path_outside_task_directory(tmp_path):
    task = _task(tmp_path)
    downloader = _downloader(task)
    (downloader._seg_dir() / "000000.seg").write_bytes(b"media")
    outside = tmp_path / "outside.init"
    outside.write_bytes(b"must-not-be-used")
    restored: list[dict] = []

    duration = downloader._restore_live_segments({
        "version": 1,
        "segments": [{
            "index": 0,
            "url": "https://example.test/part.m4s",
            "duration": 4.0,
            "media_sequence": 1,
            "init_path": str(outside),
        }],
    }, restored)

    assert duration == 0
    assert restored == []


def test_stale_window_replay_is_not_treated_as_sequence_reset(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    url = "https://example.test/live.m3u8"
    polls = {"count": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target == url:
            polls["count"] += 1
            if polls["count"] == 1:
                return httpx.Response(200, text=_live_playlist(100, ["a0.ts", "a1.ts"]))
            if polls["count"] == 2:
                # A desynced CDN edge replays an old window: same URLs, far
                # lower sequence numbers.  This must NOT count as an encoder
                # restart, or the content would be recorded twice.
                return httpx.Response(200, text=_live_playlist(90, ["a0.ts", "a1.ts"]))
            return httpx.Response(200, text=_live_playlist(102, ["a2.ts"], ended=True))
        return httpx.Response(200, content=b"seg")

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, _live_playlist(100, ["a0.ts", "a1.ts"]))
            result = await downloader._record_live(client, parsed, {}, None)
        assert result is not None
        segments, _total = result
        names = [segment["url"].rsplit("/", 1)[-1] for segment in segments]
        assert names == ["a0.ts", "a1.ts", "a2.ts"]

    asyncio.run(run())


def test_event_backlog_is_truncated_at_duration_cap(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    monkeypatch.setattr(settings, "live_record_max_minutes", 1, raising=False)
    url = "https://example.test/live.m3u8"
    downloaded: list[str] = []
    # An event playlist listing a 200-second backlog against a 60-second cap.
    playlist = (
        LIVE_HEAD
        + "#EXT-X-MEDIA-SEQUENCE:0\n"
        + "".join(f"#EXTINF:40,\nlong{i}.ts\n" for i in range(5))
    )

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target == url:
            return httpx.Response(200, text=playlist)
        downloaded.append(target.rsplit("/", 1)[-1])
        return httpx.Response(200, content=b"y" * 16)

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            result = await downloader._record_live(
                client, _parsed(url, playlist), {}, None
            )
        assert result is not None
        segments, total_duration = result
        # Only the segments needed to reach the cap are fetched; the rest of
        # the backlog is never downloaded, so the output honors the setting.
        assert len(segments) == 2
        assert total_duration == pytest.approx(80.0)
        assert sorted(downloaded) == ["long0.ts", "long1.ts"]
        assert "上限" in task.last_log

    asyncio.run(run())


def test_full_run_records_live_stream_to_final_file(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    url = "https://example.test/live.m3u8"
    polls = {"count": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target == url:
            polls["count"] += 1
            if polls["count"] == 1:
                return httpx.Response(200, text=_live_playlist(0, ["s0.ts"]))
            return httpx.Response(
                200, text=_live_playlist(0, ["s0.ts", "s1.ts"], ended=True)
            )
        return httpx.Response(200, content=b"live-bytes")

    monkeypatch.setattr(
        hls_module, "_create_hls_client",
        lambda *_args: httpx.AsyncClient(transport=httpx.MockTransport(handler)),
    )

    async def fake_merge(*, seg_dir, output_path, segments, **kwargs):
        payload = b"".join(
            (seg_dir / f"{segment['index']:06d}.seg").read_bytes()
            for segment in segments
        )
        Path(output_path).write_bytes(payload)

    monkeypatch.setattr(hls_module, "merge_segments", fake_merge)

    async def run():
        task = _task(tmp_path, url)
        downloader = HLSDownloader(task)
        await downloader.run()
        assert task.status is TaskStatus.DONE
        assert task.engine_state.get("live") is True
        output = Path(task.output_path)
        assert output.exists()
        assert output.read_bytes() == b"live-bytes" * 2
        assert task.progress.total_segments == 2
        assert task.progress.media_duration == pytest.approx(8.0)

    asyncio.run(run())


def test_live_recording_survives_media_sequence_reset(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    url = "https://example.test/live.m3u8"
    polls = {"count": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target == url:
            polls["count"] += 1
            if polls["count"] == 1:
                return httpx.Response(200, text=_live_playlist(100, ["a0.ts", "a1.ts"]))
            # Encoder restart: numbering starts over with fresh content.
            return httpx.Response(
                200, text=_live_playlist(0, ["b0.ts", "b1.ts"], ended=True)
            )
        return httpx.Response(200, content=b"seg")

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, _live_playlist(100, ["a0.ts", "a1.ts"]))
            result = await downloader._record_live(client, parsed, {}, None)
        assert result is not None
        segments, _total = result
        names = [segment["url"].rsplit("/", 1)[-1] for segment in segments]
        assert names == ["a0.ts", "a1.ts", "b0.ts", "b1.ts"]
        # The first segment of the new epoch is marked as a timeline break.
        assert segments[2]["discontinuity"] is True

    asyncio.run(run())


def test_live_recording_ignores_rotated_url_tokens(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    url = "https://example.test/live.m3u8"
    polls = {"count": 0}

    def playlist_with_token(token: str, ended: bool) -> str:
        lines = [LIVE_HEAD + "#EXT-X-MEDIA-SEQUENCE:0"]
        for name in ("s0.ts", "s1.ts"):
            lines.append("#EXTINF:4,")
            lines.append(f"{name}?auth={token}")
        if ended:
            lines.append("#EXT-X-ENDLIST")
        return "\n".join(lines) + "\n"

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target == url:
            polls["count"] += 1
            if polls["count"] == 1:
                return httpx.Response(200, text=playlist_with_token("aaa", False))
            # Same media sequences, rotated signing token: nothing is new.
            return httpx.Response(200, text=playlist_with_token("bbb", True))
        return httpx.Response(200, content=b"seg")

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, playlist_with_token("aaa", False))
            result = await downloader._record_live(client, parsed, {}, None)
        assert result is not None
        segments, total_duration = result
        assert len(segments) == 2
        assert total_duration == pytest.approx(8.0)

    asyncio.run(run())


def test_resume_purges_orphan_segment_files_from_crashed_batch(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    url = "https://example.test/live.m3u8"

    def handler(request: httpx.Request) -> httpx.Response:
        if str(request.url) == url:
            return httpx.Response(200, text=_live_playlist(5, ["fresh.ts"], ended=True))
        return httpx.Response(200, content=b"fresh-bytes")

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        seg_dir = downloader._seg_dir()
        (seg_dir / "000000.seg").write_bytes(b"persisted")
        # Crash artifact: downloaded after the last live_state.json write.
        (seg_dir / "000001.seg").write_bytes(b"stale-crash-bytes")
        state = {
            "version": 1,
            "total_duration": 4.0,
            "segments": [
                {"index": 0, "url": "https://example.test/s0.ts", "duration": 4.0,
                 "media_sequence": 0, "discontinuity": False, "init_path": ""},
            ],
        }
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, _live_playlist(5, ["fresh.ts"], ended=True))
            result = await downloader._record_live(client, parsed, {}, state)
        assert result is not None
        segments, _total = result
        assert [segment["index"] for segment in segments] == [0, 1]
        # Index 1 must contain the freshly downloaded segment, not the stale
        # bytes left behind by the crashed batch.
        assert (seg_dir / "000001.seg").read_bytes() == b"fresh-bytes"

    asyncio.run(run())


def test_stop_request_during_manifest_reload_finalizes_instead_of_interrupting(
    tmp_path, monkeypatch
):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    url = "https://example.test/live.m3u8"
    polls = {"count": 0}
    tasks: dict[str, Task] = {}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target == url:
            polls["count"] += 1
            if polls["count"] == 1:
                return httpx.Response(200, text=_live_playlist(0, ["s0.ts"]))
            # The stop request lands while the manifest refresh is failing.
            tasks["task"].pause_event.set()
            return httpx.Response(503, request=request)
        return httpx.Response(200, content=b"seg")

    async def run():
        task = _task(tmp_path, url)
        tasks["task"] = task
        downloader = _downloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, _live_playlist(0, ["s0.ts"]))
            result = await downloader._record_live(client, parsed, {}, None)
        assert result is not None
        segments, _total = result
        assert len(segments) == 1
        assert "停止" in task.last_log

    asyncio.run(run())


def test_manifest_refresh_failure_finalizes_captured_content(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    monkeypatch.setattr(hls_module, "retry_delay_seconds", lambda *_args: 0)
    url = "https://example.test/live.m3u8"
    polls = {"count": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target == url:
            polls["count"] += 1
            if polls["count"] == 1:
                return httpx.Response(200, text=_live_playlist(0, ["s0.ts"]))
            # The stream ended and the origin removed the manifest entirely.
            return httpx.Response(404, request=request)
        return httpx.Response(200, content=b"seg")

    async def run():
        task = _task(tmp_path, url)
        downloader = _downloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, _live_playlist(0, ["s0.ts"]))
            result = await downloader._record_live(client, parsed, {}, None)
        # Captured content is finalized for merge instead of failing the task.
        assert result is not None
        segments, total_duration = result
        assert len(segments) == 1
        assert total_duration == pytest.approx(4.0)
        assert "不可用" in task.last_log

    asyncio.run(run())


def test_external_cancel_during_pending_stop_is_not_swallowed(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    monkeypatch.setattr(hls_module, "retry_delay_seconds", lambda *_args: 0)
    url = "https://example.test/live.m3u8"
    polls = {"count": 0}
    holder: dict = {}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target == url:
            polls["count"] += 1
            if polls["count"] == 1:
                return httpx.Response(200, text=_live_playlist(0, ["s0.ts"]))
            # App shutdown lands while a stop request is already pending:
            # the real cancellation must propagate instead of being read as
            # the stop, or shutdown would block on a full merge.
            holder["task"].pause_event.set()
            holder["runner"].cancel()
            return httpx.Response(503, request=request)
        return httpx.Response(200, content=b"seg")

    async def run():
        task = _task(tmp_path, url)
        holder["task"] = task
        downloader = _downloader(task)
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as client:
            parsed = _parsed(url, _live_playlist(0, ["s0.ts"]))
            holder["runner"] = asyncio.ensure_future(
                downloader._record_live(client, parsed, {}, None)
            )
            with pytest.raises(asyncio.CancelledError):
                await holder["runner"]

    asyncio.run(run())


def test_failed_live_recording_keeps_captured_segments(tmp_path):
    async def run():
        task = _task(tmp_path)
        task.engine_state["live"] = True
        downloader = _downloader(task)
        seg_dir = downloader._seg_dir()
        (seg_dir / "000000.seg").write_bytes(b"precious")
        (downloader._task_dir() / "live_state.json").write_text("{}", encoding="utf-8")
        await downloader._cleanup_failed_temp(downloader._task_dir())
        assert (seg_dir / "000000.seg").exists()
        assert (downloader._task_dir() / "live_state.json").exists()

    asyncio.run(run())


def test_full_run_merges_saved_recording_when_manifest_is_gone(tmp_path, monkeypatch):
    url = "https://example.test/live.m3u8"

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(404, request=request)

    monkeypatch.setattr(
        hls_module, "_create_hls_client",
        lambda *_args: httpx.AsyncClient(transport=httpx.MockTransport(handler)),
    )
    monkeypatch.setattr(
        hls_module, "retry_delay_seconds", lambda *_args: 0
    )

    async def fake_merge(*, seg_dir, output_path, segments, **kwargs):
        payload = b"".join(
            (seg_dir / f"{segment['index']:06d}.seg").read_bytes()
            for segment in segments
        )
        Path(output_path).write_bytes(payload)

    monkeypatch.setattr(hls_module, "merge_segments", fake_merge)

    async def run():
        task = _task(tmp_path, url)
        task.engine_state["live"] = True
        downloader = HLSDownloader(task)
        seg_dir = downloader._seg_dir()
        seg_dir.mkdir(parents=True, exist_ok=True)
        (seg_dir / "000000.seg").write_bytes(b"first")
        (seg_dir / "000001.seg").write_bytes(b"second")
        state = {
            "version": 1,
            "total_duration": 8.0,
            "segments": [
                {"index": 0, "url": "https://example.test/s0.ts", "duration": 4.0,
                 "media_sequence": 0, "discontinuity": False, "init_path": ""},
                {"index": 1, "url": "https://example.test/s1.ts", "duration": 4.0,
                 "media_sequence": 1, "discontinuity": False, "init_path": ""},
            ],
        }
        (downloader._task_dir() / "live_state.json").write_text(
            json.dumps(state), encoding="utf-8"
        )
        await downloader.run()
        assert task.status is TaskStatus.DONE
        output = Path(task.output_path)
        assert output.read_bytes() == b"firstsecond"
        assert task.progress.media_duration == pytest.approx(8.0)

    asyncio.run(run())


def test_full_run_records_and_muxes_live_master_with_external_audio(tmp_path, monkeypatch):
    monkeypatch.setattr(HLSDownloader, "_live_wait", _instant_wait)
    master_url = "https://example.test/master.m3u8"
    video_url = "https://example.test/video.m3u8"
    audio_url = "https://example.test/audio.m3u8"
    master = """#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aud",NAME="chinese",URI="audio.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=2000,AUDIO="aud"
video.m3u8
"""
    polls = {video_url: 0, audio_url: 0}

    def handler(request: httpx.Request) -> httpx.Response:
        target = str(request.url)
        if target == master_url:
            return httpx.Response(200, text=master)
        if target in polls:
            polls[target] += 1
            name = "video.ts" if target == video_url else "audio.aac"
            return httpx.Response(
                200,
                text=_live_playlist(0, [name], ended=polls[target] > 1),
            )
        if target == "https://example.test/video.ts":
            return httpx.Response(200, content=b"video")
        if target == "https://example.test/audio.aac":
            return httpx.Response(200, content=b"audio")
        return httpx.Response(404)

    monkeypatch.setattr(
        hls_module, "_create_hls_client",
        lambda *_args: httpx.AsyncClient(transport=httpx.MockTransport(handler)),
    )

    async def fake_merge(*, seg_dir, output_path, segments, **kwargs):
        payload = b"".join(
            (seg_dir / f"{segment['index']:06d}.seg").read_bytes()
            for segment in segments
        )
        Path(output_path).parent.mkdir(parents=True, exist_ok=True)
        Path(output_path).write_bytes(payload)

    async def fake_mux(*, video_path, audio_path, output_path, **kwargs):
        payload = Path(video_path).read_bytes() + b"+" + Path(audio_path).read_bytes()
        Path(output_path).write_bytes(payload)

    monkeypatch.setattr(hls_module, "merge_segments", fake_merge)
    monkeypatch.setattr(hls_module, "mux_media_tracks", fake_mux)

    async def run():
        task = _task(tmp_path, master_url)
        downloader = HLSDownloader(task)
        await downloader.run()
        assert task.status is TaskStatus.DONE
        assert Path(task.output_path).read_bytes() == b"video+audio"
        assert polls[video_url] >= 2
        assert polls[audio_url] >= 1

    asyncio.run(run())
