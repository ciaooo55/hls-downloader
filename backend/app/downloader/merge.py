import asyncio
import contextlib
import json
import logging
from pathlib import Path

from ..models import TaskStatus


PREPARE_PROGRESS_END = 30.0
FFMPEG_PROGRESS_END = 98.0
STDERR_TAIL_LIMIT = 64 * 1024
logger = logging.getLogger(__name__)


def _emit_progress(task, on_progress) -> None:
    if task is not None and on_progress is not None:
        on_progress(task)


def _combine_files(init_path: Path, segment_path: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(destination.name + ".tmp")
    try:
        with temporary.open("wb") as output:
            for source_path in (init_path, segment_path):
                with source_path.open("rb") as source:
                    while chunk := source.read(1024 * 1024):
                        output.write(chunk)
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)


def _concat_line(path: Path) -> str:
    escaped = str(path.resolve()).replace("\\", "/").replace("'", "'\\''")
    return f"file '{escaped}'"


async def merge_segments(
    seg_dir: Path,
    output_path: Path,
    segments: list[dict],
    ffmpeg_path: str,
    task=None,
    total_duration: float = 0,
    on_progress=None,
) -> None:
    if not segments:
        raise ValueError("没有可合并的分片")

    prepared_dir = seg_dir / "prepared"
    concat_inputs: list[Path] = []
    for position, segment in enumerate(segments):
        index = int(segment["index"])
        segment_path = seg_dir / f"{index:06d}.seg"
        if not segment_path.exists() or segment_path.stat().st_size == 0:
            raise FileNotFoundError(f"缺少分片: {segment_path.name}")

        init_path_text = segment.get("init_path")
        if init_path_text:
            init_path = Path(init_path_text)
            if not init_path.exists() or init_path.stat().st_size == 0:
                raise FileNotFoundError(f"缺少 init map: {init_path}")
            prepared = prepared_dir / f"{index:06d}.part"
            await asyncio.to_thread(
                _combine_files,
                init_path,
                segment_path,
                prepared,
            )
            concat_inputs.append(prepared)
        else:
            concat_inputs.append(segment_path)

        if task is not None:
            percent = ((position + 1) / len(segments)) * PREPARE_PROGRESS_END
            task.status = TaskStatus.MERGING
            task.stage = "merging"
            task.progress.post_percent = percent
            task.last_log = (
                f"准备合并 {position + 1}/{len(segments)} ({percent:.1f}%)"
            )
            _emit_progress(task, on_progress)
        await asyncio.sleep(0)

    concat_path = seg_dir.parent / "concat.txt"
    concat_path.write_text(
        "\n".join(_concat_line(path) for path in concat_inputs) + "\n",
        encoding="utf-8",
    )

    if task is not None:
        task.status = TaskStatus.REMUXING
        task.stage = "remuxing"
        task.progress.post_percent = PREPARE_PROGRESS_END
        task.last_log = "ffmpeg 正在转封装"
        _emit_progress(task, on_progress)

    temporary_output = output_path.with_name(
        f"{output_path.stem}.merging{output_path.suffix or '.tmp'}"
    )
    temporary_output.unlink(missing_ok=True)
    copy_command = [
        ffmpeg_path,
        "-y",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        str(concat_path),
        "-c",
        "copy",
        "-movflags",
        "+faststart",
        "-progress",
        "pipe:1",
        "-nostats",
        str(temporary_output),
    ]
    try:
        success = await _run_ffmpeg(
            copy_command,
            task=task,
            duration_sec=total_duration,
            on_progress=on_progress,
        )
        if not success:
            temporary_output.unlink(missing_ok=True)
            if task is not None:
                task.last_log = "无损转封装失败，正在尝试重新编码"
                _emit_progress(task, on_progress)
            encode_command = [
                ffmpeg_path,
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                str(concat_path),
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
                "-progress",
                "pipe:1",
                "-nostats",
                str(temporary_output),
            ]
            success = await _run_ffmpeg(
                encode_command,
                task=task,
                duration_sec=total_duration,
                on_progress=on_progress,
            )
        if not success:
            if task is not None and task.last_log.startswith("ffmpeg"):
                raise RuntimeError(task.last_log)
            raise RuntimeError("ffmpeg 合并失败，未返回可读取的错误信息")

        if task is not None:
            task.stage = "verifying"
            task.progress.post_percent = 99.0
            task.last_log = "正在验证输出文件"
            _emit_progress(task, on_progress)
        await _verify_output(ffmpeg_path, temporary_output, total_duration)
        temporary_output.replace(output_path)
    finally:
        temporary_output.unlink(missing_ok=True)

    if task is not None:
        task.progress.post_percent = 100.0
        task.last_log = f"后处理完成: {output_path.name}"
        _emit_progress(task, on_progress)


async def _terminate_process(process: asyncio.subprocess.Process) -> None:
    if process.returncode is not None:
        return
    with contextlib.suppress(ProcessLookupError):
        process.terminate()
    try:
        await asyncio.wait_for(process.wait(), timeout=5)
    except asyncio.TimeoutError:
        with contextlib.suppress(ProcessLookupError):
            process.kill()
        await process.wait()


async def _run_ffmpeg(
    command: list[str],
    task=None,
    duration_sec: float = 0,
    on_progress=None,
) -> bool:
    process: asyncio.subprocess.Process | None = None
    stderr_tail = bytearray()
    stderr_task: asyncio.Task | None = None

    async def read_stderr() -> None:
        while True:
            chunk = await process.stderr.read(4096)
            if not chunk:
                return
            stderr_tail.extend(chunk)
            if len(stderr_tail) > STDERR_TAIL_LIMIT:
                del stderr_tail[:-STDERR_TAIL_LIMIT]

    try:
        process = await asyncio.create_subprocess_exec(
            *command,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stderr_task = asyncio.create_task(read_stderr())
        while True:
            line = await asyncio.wait_for(process.stdout.readline(), timeout=600)
            if not line:
                break
            key, separator, value = line.decode("utf-8", errors="replace").strip().partition("=")
            if separator and key in {"out_time_us", "out_time_ms"} and duration_sec > 0:
                try:
                    current_seconds = int(value) / 1_000_000
                except ValueError:
                    continue
                ratio = max(0.0, min(1.0, current_seconds / duration_sec))
                percent = PREPARE_PROGRESS_END + ratio * (
                    FFMPEG_PROGRESS_END - PREPARE_PROGRESS_END
                )
                if task is not None and abs(percent - task.progress.post_percent) >= 0.1:
                    task.progress.post_percent = percent
                    task.last_log = (
                        f"ffmpeg {percent:.1f}% "
                        f"({_fmt_time(current_seconds)}/{_fmt_time(duration_sec)})"
                    )
                    _emit_progress(task, on_progress)

        return_code = await process.wait()
        if stderr_task:
            await stderr_task
        if return_code != 0:
            error_text = stderr_tail.decode("utf-8", errors="replace").strip()
            if task is not None:
                task.last_log = f"ffmpeg 失败: {error_text[-500:]}"
                _emit_progress(task, on_progress)
            else:
                logger.error("ffmpeg failed: %s", error_text[-500:])
            return False
        return True
    except asyncio.CancelledError:
        if process is not None:
            await _terminate_process(process)
        raise
    except asyncio.TimeoutError:
        if process is not None:
            await _terminate_process(process)
        if task is not None:
            task.last_log = "ffmpeg 超过 600 秒没有输出，已终止"
            _emit_progress(task, on_progress)
        return False
    except Exception as exc:
        if process is not None:
            await _terminate_process(process)
        if task is not None:
            task.last_log = f"ffmpeg 启动失败: {exc}"
            _emit_progress(task, on_progress)
        else:
            logger.exception("ffmpeg exception")
        return False
    finally:
        if stderr_task and not stderr_task.done():
            stderr_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await stderr_task


def _ffprobe_path(ffmpeg_path: str) -> str:
    path = Path(ffmpeg_path)
    suffix = path.suffix or (".exe" if path.name.lower().endswith(".exe") else "")
    return str(path.with_name(f"ffprobe{suffix}"))


async def _probe_duration(ffmpeg_path: str, input_file: Path) -> float:
    try:
        process = await asyncio.create_subprocess_exec(
            _ffprobe_path(ffmpeg_path),
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            str(input_file),
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, _ = await asyncio.wait_for(process.communicate(), timeout=30)
        if process.returncode != 0:
            return 0.0
        data = json.loads(stdout.decode("utf-8", errors="replace"))
        return float(data.get("format", {}).get("duration", 0) or 0)
    except Exception:
        return 0.0


async def _verify_output(
    ffmpeg_path: str,
    output_path: Path,
    expected_duration: float,
) -> None:
    if not output_path.exists() or output_path.stat().st_size == 0:
        raise RuntimeError("输出文件为空或不存在")
    actual_duration = await _probe_duration(ffmpeg_path, output_path)
    if expected_duration >= 3 and actual_duration > 0:
        minimum = expected_duration * 0.9
        if actual_duration < minimum:
            raise RuntimeError(
                f"输出时长异常，期望约 {expected_duration:.1f}s，实际 {actual_duration:.1f}s"
            )


def _fmt_time(seconds: float) -> str:
    minutes, second = divmod(int(seconds), 60)
    hour, minute = divmod(minutes, 60)
    return f"{hour}:{minute:02d}:{second:02d}"
