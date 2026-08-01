import asyncio
import shutil
import subprocess
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

from backend.app.downloader import merge as merge_mod


def _task():
    return SimpleNamespace(
        status="merging",
        stage="",
        progress=SimpleNamespace(post_percent=0.0),
        last_log="",
        cancel_event=asyncio.Event(),
    )


def test_merge_segments_builds_local_hls_timeline_and_emits_progress(tmp_path, monkeypatch):
    seg_dir = tmp_path / "segments"
    seg_dir.mkdir()
    init_path = tmp_path / "init.mp4"
    init_path.write_bytes(b"init-")
    (seg_dir / "000000.seg").write_bytes(b"one")
    (seg_dir / "000001.seg").write_bytes(b"two")
    output_path = tmp_path / "out.mp4"
    output_path.touch()
    task = _task()
    updates = []
    captured_playlist = []

    async def fake_run_ffmpeg(
        cmd,
        task=None,
        duration_sec=0,
        on_progress=None,
    ):
        playlist_path = Path(cmd[cmd.index("-i") + 1])
        captured_playlist.extend(playlist_path.read_text(encoding="utf-8").splitlines())
        Path(cmd[-1]).write_bytes(b"mp4")
        return True

    monkeypatch.setattr(merge_mod, "_run_ffmpeg", fake_run_ffmpeg)
    monkeypatch.setattr(merge_mod, "_probe_duration", _async_positive)

    segments = [
        {"index": 0, "init_path": str(init_path), "duration": 4},
        {"index": 1, "init_path": str(init_path), "duration": 5, "discontinuity": True},
    ]
    asyncio.run(
        merge_mod.merge_segments(
            seg_dir=seg_dir,
            output_path=output_path,
            segments=segments,
            ffmpeg_path="ffmpeg",
            task=task,
            total_duration=9,
            on_progress=lambda current: updates.append(
                (current.progress.post_percent, current.last_log)
            ),
        )
    )

    assert "#EXT-X-MAP:" in "\n".join(captured_playlist)
    assert "#EXT-X-DISCONTINUITY" in captured_playlist
    assert any(line.endswith("segments/000000.seg") for line in captured_playlist)
    assert any(line.endswith("segments/000001.seg") for line in captured_playlist)
    assert "concat" not in captured_playlist
    assert any("准备" in log for _, log in updates)
    assert any("ffmpeg" in log for _, log in updates)


def test_fmp4_playlist_creation_does_not_block_event_loop(tmp_path, monkeypatch):
    seg_dir = tmp_path / "segments"
    seg_dir.mkdir()
    init_path = tmp_path / "init.mp4"
    init_path.write_bytes(b"init")
    (seg_dir / "000000.seg").write_bytes(b"segment")
    output_path = tmp_path / "out.mp4"
    output_path.touch()
    task = _task()
    original_write_playlist = merge_mod._write_local_hls_playlist

    def slow_write_playlist(*args):
        time.sleep(0.05)
        return original_write_playlist(*args)

    async def fake_run_ffmpeg(cmd, task=None, duration_sec=0, on_progress=None):
        Path(cmd[-1]).write_bytes(b"mp4")
        return True

    monkeypatch.setattr(merge_mod, "_write_local_hls_playlist", slow_write_playlist)
    monkeypatch.setattr(merge_mod, "_run_ffmpeg", fake_run_ffmpeg)
    monkeypatch.setattr(merge_mod, "_probe_duration", _async_positive)

    async def run():
        ticks = 0
        finished = False

        async def ticker():
            nonlocal ticks
            while not finished:
                ticks += 1
                await asyncio.sleep(0.005)

        ticker_task = asyncio.create_task(ticker())
        await merge_mod.merge_segments(
            seg_dir=seg_dir,
            output_path=output_path,
            segments=[
                {"index": 0, "init_path": str(init_path), "duration": 1},
            ],
            ffmpeg_path="ffmpeg",
            task=task,
            total_duration=1,
        )
        finished = True
        await ticker_task
        assert ticks >= 3

    asyncio.run(run())


def test_durable_output_publish_does_not_block_event_loop(tmp_path, monkeypatch):
    seg_dir = tmp_path / "segments"
    seg_dir.mkdir()
    (seg_dir / "000000.seg").write_bytes(b"segment")
    output_path = tmp_path / "out.mp4"
    output_path.touch()
    original_replace = merge_mod.durable_replace

    async def fake_run_ffmpeg(cmd, **_kwargs):
        Path(cmd[-1]).write_bytes(b"mp4")
        return True

    def slow_replace(*args):
        time.sleep(0.06)
        return original_replace(*args)

    monkeypatch.setattr(merge_mod, "_run_ffmpeg", fake_run_ffmpeg)
    monkeypatch.setattr(merge_mod, "_probe_duration", _async_positive)
    monkeypatch.setattr(merge_mod, "durable_replace", slow_replace)

    async def run():
        ticks = 0
        finished = False

        async def ticker():
            nonlocal ticks
            while not finished:
                ticks += 1
                await asyncio.sleep(0.005)

        ticker_task = asyncio.create_task(ticker())
        await merge_mod.merge_segments(
            seg_dir=seg_dir,
            output_path=output_path,
            segments=[{"index": 0, "init_path": None, "duration": 1}],
            ffmpeg_path="ffmpeg",
            total_duration=1,
        )
        finished = True
        await ticker_task
        assert ticks >= 5

    asyncio.run(run())


def test_merge_segments_writes_temp_output_then_replaces_placeholder(tmp_path, monkeypatch):
    seg_dir = tmp_path / "segments"
    seg_dir.mkdir()
    (seg_dir / "000000.seg").write_bytes(b"one")
    output_path = tmp_path / "out.mp4"
    output_path.touch()
    ffmpeg_outputs = []

    async def fake_run_ffmpeg(cmd, task=None, duration_sec=0, on_progress=None):
        actual_output = Path(cmd[-1])
        assert actual_output.suffix == ".mp4"
        ffmpeg_outputs.append(actual_output)
        actual_output.write_bytes(b"mp4")
        return True

    async def fake_verify(ffmpeg_path, path, total_duration=0):
        assert Path(path) != output_path
        assert Path(path).read_bytes() == b"mp4"

    monkeypatch.setattr(merge_mod, "_run_ffmpeg", fake_run_ffmpeg)
    monkeypatch.setattr(merge_mod, "_verify_output", fake_verify)

    asyncio.run(
        merge_mod.merge_segments(
            seg_dir=seg_dir,
            output_path=output_path,
            segments=[{"index": 0, "init_path": None, "duration": 1}],
            ffmpeg_path="ffmpeg",
            total_duration=1,
        )
    )

    assert ffmpeg_outputs == [output_path.with_name("out.merging.mp4")]
    assert output_path.read_bytes() == b"mp4"
    assert not output_path.with_name("out.merging.mp4").exists()


def test_merge_failure_preserves_ffmpeg_stderr_reason(tmp_path, monkeypatch):
    seg_dir = tmp_path / "segments"
    seg_dir.mkdir()
    (seg_dir / "000000.seg").write_bytes(b"one")
    output_path = tmp_path / "out.mp4"
    output_path.touch()
    task = _task()

    async def fake_run_ffmpeg(cmd, task=None, duration_sec=0, on_progress=None):
        task.last_log = "ffmpeg 失败: Invalid data found when processing input"
        return False

    monkeypatch.setattr(merge_mod, "_run_ffmpeg", fake_run_ffmpeg)

    with pytest.raises(RuntimeError, match="Invalid data found"):
        asyncio.run(
            merge_mod.merge_segments(
                seg_dir=seg_dir,
                output_path=output_path,
                segments=[{"index": 0, "init_path": None, "duration": 1}],
                ffmpeg_path="ffmpeg",
                task=task,
                total_duration=1,
            )
        )


def test_implausible_copy_timeline_falls_back_to_reencode(tmp_path, monkeypatch):
    seg_dir = tmp_path / "segments"
    seg_dir.mkdir()
    (seg_dir / "000000.seg").write_bytes(b"one")
    output_path = tmp_path / "out.mp4"
    output_path.touch()
    commands = []
    probed = 0

    async def fake_run_ffmpeg(command, **_kwargs):
        commands.append(command)
        Path(command[-1]).write_bytes(b"media")
        return True

    async def probe(_ffmpeg, _path):
        nonlocal probed
        probed += 1
        return 100.0 if probed == 1 else 4.0

    monkeypatch.setattr(merge_mod, "_run_ffmpeg", fake_run_ffmpeg)
    monkeypatch.setattr(merge_mod, "_probe_duration", probe)

    asyncio.run(
        merge_mod.merge_segments(
            seg_dir=seg_dir,
            output_path=output_path,
            segments=[{"index": 0, "duration": 4}],
            ffmpeg_path="ffmpeg",
            total_duration=4,
        )
    )

    assert len(commands) == 2
    assert "copy" in commands[0]
    assert "libx264" in commands[1]
    assert output_path.read_bytes() == b"media"


def test_mux_media_tracks_maps_video_and_external_audio(tmp_path, monkeypatch):
    video = tmp_path / "video.mp4"
    audio = tmp_path / "audio.mp4"
    output = tmp_path / "output.mp4"
    video.write_bytes(b"video")
    audio.write_bytes(b"audio")
    output.write_bytes(b"video-only")
    commands = []

    async def fake_run_ffmpeg(command, **kwargs):
        commands.append(command)
        Path(command[-1]).write_bytes(b"muxed")
        return True

    async def fake_verify(_ffmpeg, path, _duration):
        assert Path(path).read_bytes() == b"muxed"

    monkeypatch.setattr(merge_mod, "_run_ffmpeg", fake_run_ffmpeg)
    monkeypatch.setattr(merge_mod, "_verify_output", fake_verify)

    asyncio.run(merge_mod.mux_media_tracks(
        video_path=video,
        audio_path=audio,
        output_path=output,
        ffmpeg_path="ffmpeg",
        total_duration=4,
    ))

    assert output.read_bytes() == b"muxed"
    assert ["-map", "0:v:0"] == commands[0][commands[0].index("-map"):commands[0].index("-map") + 2]
    assert "1:a:0" in commands[0]
    assert commands[0][commands[0].index("-t") + 1] == "4.000000"
    assert not output.with_name("output.muxing.mp4").exists()


def test_verify_output_rejects_media_that_ffprobe_cannot_read(tmp_path, monkeypatch):
    output = tmp_path / "broken.mp4"
    output.write_bytes(b"not-a-media-file")
    monkeypatch.setattr(merge_mod, "_probe_duration", _async_zero)

    with pytest.raises(RuntimeError, match="ffprobe 无法读取"):
        asyncio.run(merge_mod._verify_output("ffmpeg", output, 10))


def test_verify_output_rejects_implausibly_long_timeline(tmp_path, monkeypatch):
    output = tmp_path / "overlong.mp4"
    output.write_bytes(b"media")

    async def overlong(*_args, **_kwargs):
        return 3600.0

    monkeypatch.setattr(merge_mod, "_probe_duration", overlong)

    with pytest.raises(RuntimeError, match="输出时长异常"):
        asyncio.run(merge_mod._verify_output("ffmpeg", output, 60))


@pytest.mark.skipif(shutil.which("ffmpeg") is None, reason="ffmpeg is unavailable")
def test_real_fmp4_hls_merge_keeps_manifest_timeline(tmp_path):
    """Later fMP4 fragments carry absolute tfdt values, not durations."""
    ffmpeg = str(shutil.which("ffmpeg"))
    source_playlist = tmp_path / "source.m3u8"
    subprocess.run(
        [
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x90:rate=25:duration=4",
            "-c:v",
            "libx264",
            "-g",
            "25",
            "-keyint_min",
            "25",
            "-sc_threshold",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "hls",
            "-hls_time",
            "1",
            "-hls_list_size",
            "0",
            "-hls_segment_type",
            "fmp4",
            "-hls_flags",
            "independent_segments",
            str(source_playlist),
        ],
        cwd=tmp_path,
        check=True,
        capture_output=True,
    )

    lines = source_playlist.read_text(encoding="utf-8").splitlines()
    init_name = next(
        line.split('URI="', 1)[1].split('"', 1)[0]
        for line in lines
        if line.startswith("#EXT-X-MAP:")
    )
    media = [
        (float(line.split(":", 1)[1].split(",", 1)[0]), lines[index + 1])
        for index, line in enumerate(lines)
        if line.startswith("#EXTINF:")
    ]
    seg_dir = tmp_path / "segments"
    seg_dir.mkdir()
    segments = []
    for index, (duration, name) in enumerate(media):
        (seg_dir / f"{index:06d}.seg").write_bytes((tmp_path / name).read_bytes())
        segments.append(
            {
                "index": index,
                "duration": duration,
                "init_path": str(tmp_path / init_name),
            }
        )

    expected = sum(item["duration"] for item in segments)
    output = tmp_path / "merged.mp4"
    output.touch()
    asyncio.run(
        merge_mod.merge_segments(
            seg_dir=seg_dir,
            output_path=output,
            segments=segments,
            ffmpeg_path=ffmpeg,
            total_duration=expected,
        )
    )

    actual = asyncio.run(merge_mod._probe_duration(ffmpeg, output))
    assert actual == pytest.approx(expected, abs=0.25)


@pytest.mark.skipif(shutil.which("ffmpeg") is None, reason="ffmpeg is unavailable")
def test_real_external_audio_mux_is_capped_to_video_timeline(tmp_path):
    ffmpeg = str(shutil.which("ffmpeg"))
    video = tmp_path / "video.mp4"
    audio = tmp_path / "audio.m4a"
    output = tmp_path / "muxed.mp4"
    subprocess.run(
        [
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x90:rate=25:duration=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            str(video),
        ],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        [
            ffmpeg,
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=8",
            "-c:a",
            "aac",
            str(audio),
        ],
        check=True,
        capture_output=True,
    )
    output.touch()

    asyncio.run(
        merge_mod.mux_media_tracks(
            video_path=video,
            audio_path=audio,
            output_path=output,
            ffmpeg_path=ffmpeg,
            total_duration=2,
        )
    )

    actual = asyncio.run(merge_mod._probe_duration(ffmpeg, output))
    assert actual == pytest.approx(2.0, abs=0.25)


async def _async_zero(*args, **kwargs):
    return 0.0


async def _async_positive(*args, **kwargs):
    return 9.0


def test_ffprobe_process_start_does_not_block_api_event_loop(tmp_path, monkeypatch):
    output = tmp_path / "output.mp4"
    output.write_bytes(b"media")

    def slow_process_start(*_args, **_kwargs):
        # Models Windows Defender scanning a newly extracted ffprobe.exe while
        # CreateProcess is still synchronous.
        time.sleep(0.2)
        return SimpleNamespace(returncode=1, stdout=b"", stderr=b"")

    monkeypatch.setattr(merge_mod.subprocess, "run", slow_process_start)

    async def scenario() -> float:
        started = time.monotonic()
        probe = asyncio.create_task(merge_mod._probe_duration("ffmpeg.exe", output))
        await asyncio.sleep(0.02)
        heartbeat_latency = time.monotonic() - started
        assert await probe == 0.0
        return heartbeat_latency

    assert asyncio.run(scenario()) < 0.1
