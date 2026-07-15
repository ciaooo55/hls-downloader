import asyncio
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


def test_merge_segments_builds_concat_list_and_emits_progress(tmp_path, monkeypatch):
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
    captured_list = []

    async def fake_run_ffmpeg(
        cmd,
        task=None,
        duration_sec=0,
        on_progress=None,
    ):
        concat_path = Path(cmd[cmd.index("-i") + 1])
        captured_list.extend(concat_path.read_text(encoding="utf-8").splitlines())
        Path(cmd[-1]).write_bytes(b"mp4")
        return True

    monkeypatch.setattr(merge_mod, "_run_ffmpeg", fake_run_ffmpeg)
    monkeypatch.setattr(merge_mod, "_probe_duration", _async_positive)

    segments = [
        {"index": 0, "init_path": str(init_path), "duration": 4},
        {"index": 1, "init_path": None, "duration": 5},
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

    prepared = seg_dir / "prepared" / "000000.part"
    assert prepared.read_bytes() == b"init-one"
    assert len(captured_list) == 2
    assert "000000.part" in captured_list[0]
    assert "000001.seg" in captured_list[1]
    assert any("准备" in log for _, log in updates)
    assert any("ffmpeg" in log for _, log in updates)


def test_fmp4_preparation_does_not_block_event_loop(tmp_path, monkeypatch):
    seg_dir = tmp_path / "segments"
    seg_dir.mkdir()
    init_path = tmp_path / "init.mp4"
    init_path.write_bytes(b"init")
    (seg_dir / "000000.seg").write_bytes(b"segment")
    output_path = tmp_path / "out.mp4"
    output_path.touch()
    task = _task()
    original_combine = merge_mod._combine_files

    def slow_combine(*args):
        time.sleep(0.05)
        return original_combine(*args)

    async def fake_run_ffmpeg(cmd, task=None, duration_sec=0, on_progress=None):
        Path(cmd[-1]).write_bytes(b"mp4")
        return True

    monkeypatch.setattr(merge_mod, "_combine_files", slow_combine)
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


def test_verify_output_rejects_media_that_ffprobe_cannot_read(tmp_path, monkeypatch):
    output = tmp_path / "broken.mp4"
    output.write_bytes(b"not-a-media-file")
    monkeypatch.setattr(merge_mod, "_probe_duration", _async_zero)

    with pytest.raises(RuntimeError, match="ffprobe 无法读取"):
        asyncio.run(merge_mod._verify_output("ffmpeg", output, 10))


async def _async_zero(*args, **kwargs):
    return 0.0


async def _async_positive(*args, **kwargs):
    return 9.0
