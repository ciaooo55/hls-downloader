import asyncio
import contextlib
import json
import logging
import math
import os
import subprocess
import time
from functools import lru_cache
from pathlib import Path
from urllib.parse import quote

from ..models import TaskStatus
from ..utils import durable_replace
from .disk_space import MIN_FREE_RESERVE, ensure_free_space, estimate_paths_size
from .postprocess_slot import acquire_postprocess_lease


PREPARE_PROGRESS_END = 30.0
FFMPEG_PROGRESS_END = 98.0
STDERR_TAIL_LIMIT = 64 * 1024
logger = logging.getLogger(__name__)


def _hidden_subprocess_kwargs() -> dict:
    """Prevent ffmpeg/ffprobe from flashing a console on Windows."""
    if os.name != "nt":
        return {}
    return {"creationflags": getattr(subprocess, "CREATE_NO_WINDOW", 0)}


def _emit_progress(task, on_progress) -> None:
    if task is not None and on_progress is not None:
        on_progress(task)


def _emit_log(task, on_log, message: str) -> None:
    if task is not None:
        task.last_log = message
    if task is not None and on_log is not None:
        on_log(task.id, f"[merge] {message}")


def _local_hls_uri(playlist_dir: Path, path: Path) -> str:
    """Return an FFmpeg-safe URI for one downloaded local HLS resource."""
    try:
        relative = path.resolve().relative_to(playlist_dir.resolve())
    except ValueError:
        return path.resolve().as_uri()
    return quote(relative.as_posix(), safe="/-._~")


def write_local_hls_playlist(
    destination: Path,
    seg_dir: Path,
    segments: list[dict],
    segment_suffix: str = ".seg",
) -> None:
    """Recreate HLS timeline semantics around already downloaded segments.

    Concatenating ``init.mp4 + fragment.m4s`` as independent MP4 files is not
    equivalent to HLS.  Each later fragment retains its absolute ``tfdt``
    timestamp, so FFmpeg's concat demuxer interprets that timestamp as the
    fragment's duration and produces an increasingly overlong movie.  A local
    end-listed HLS manifest preserves EXT-X-MAP and discontinuity boundaries;
    FFmpeg can then normalize the media timeline exactly as it does in a
    player, without another network request or a lossy re-encode.
    """
    durations = [max(0.0, float(item.get("duration") or 0)) for item in segments]
    target_duration = max(1, math.ceil(max(durations, default=1.0)))
    uses_init_map = any(item.get("init_path") for item in segments)
    lines = [
        "#EXTM3U",
        f"#EXT-X-VERSION:{7 if uses_init_map else 3}",
        f"#EXT-X-TARGETDURATION:{target_duration}",
        "#EXT-X-MEDIA-SEQUENCE:0",
    ]
    previous_init = ""
    for position, segment in enumerate(segments):
        index = int(segment["index"])
        segment_path = seg_dir / f"{index:06d}{segment_suffix}"
        if not segment_path.exists() or segment_path.stat().st_size == 0:
            raise FileNotFoundError(f"缺少分片: {segment_path.name}")

        init_text = str(segment.get("init_path") or "")
        init_path = Path(init_text) if init_text else None
        if init_path is not None and (
            not init_path.exists() or init_path.stat().st_size == 0
        ):
            raise FileNotFoundError(f"缺少 init map: {init_path}")

        init_changed = position > 0 and init_text != previous_init
        if position > 0 and (bool(segment.get("discontinuity")) or init_changed):
            lines.append("#EXT-X-DISCONTINUITY")
        if init_path is not None and init_text != previous_init:
            uri = _local_hls_uri(destination.parent, init_path)
            lines.append(f'#EXT-X-MAP:URI="{uri}"')
        previous_init = init_text

        duration = max(0.000001, float(segment.get("duration") or 0))
        lines.append(f"#EXTINF:{duration:.6f},")
        lines.append(_local_hls_uri(destination.parent, segment_path))
    lines.append("#EXT-X-ENDLIST")
    destination.write_text("\n".join(lines) + "\n", encoding="utf-8")


def _looks_like_mpeg_ts(path: Path) -> bool:
    """Accept only confidently identified MPEG-TS files for byte-stream concat."""
    try:
        with path.open("rb") as source:
            sample = source.read(188 * 3)
    except OSError:
        return False
    return len(sample) >= 188 * 2 and all(sample[offset] == 0x47 for offset in (0, 188))


def _can_use_mpeg_ts_concat(seg_dir: Path, segments: list[dict]) -> bool:
    """Return whether HLS timeline reconstruction is unnecessary and unsafe to use."""
    if not segments or any(item.get("init_path") or item.get("discontinuity") for item in segments):
        return False
    return all(
        _looks_like_mpeg_ts(seg_dir / f"{int(item['index']):06d}.seg")
        for item in segments
    )


def write_local_concatf_playlist(
    destination: Path,
    seg_dir: Path,
    segments: list[dict],
    segment_suffix: str = ".seg",
) -> None:
    """Write FFmpeg concatf input without creating a second multi-GB file."""
    lines = []
    for segment in segments:
        path = seg_dir / f"{int(segment['index']):06d}{segment_suffix}"
        if not path.exists() or path.stat().st_size == 0:
            raise FileNotFoundError(f"缺少分片: {path.name}")
        # concatf on the bundled Windows FFmpeg accepts ``file:D:/...``;
        # the RFC form ``file:///D:/...`` is rejected as an invalid input.
        lines.append(f"file:{path.resolve().as_posix()}")
    destination.write_text("\n".join(lines) + "\n", encoding="utf-8")


# Compatibility hook retained for existing callers/tests; new protocol
# engines should use the shared public helper above.
def _write_local_hls_playlist(
    destination: Path,
    seg_dir: Path,
    segments: list[dict],
    segment_suffix: str = ".seg",
) -> None:
    write_local_hls_playlist(destination, seg_dir, segments, segment_suffix)


@lru_cache(maxsize=8)
def _local_hls_input_options(ffmpeg_path: str) -> tuple[str, ...]:
    """Build HLS demuxer options supported by this FFmpeg executable.

    Recent FFmpeg builds validate both general resource extensions and media
    segment extensions. Older builds expose only ``allowed_extensions`` and
    fail on an unknown option, so detect the additional guard once per binary.
    """
    options = ["-allowed_extensions", "ALL"]
    try:
        result = subprocess.run(
            [ffmpeg_path, "-hide_banner", "-h", "demuxer=hls"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
            **_hidden_subprocess_kwargs(),
        )
        help_text = f"{result.stdout}\n{result.stderr}"
        if "allowed_segment_extensions" in help_text:
            options.extend(["-allowed_segment_extensions", "ALL"])
        if "extension_picky" in help_text:
            # Downloaded files intentionally use the neutral .seg suffix; the
            # bytes themselves may be MPEG-TS, AAC or fMP4. FFmpeg 8 otherwise
            # rejects a valid fMP4 fragment because its suffix differs from the
            # init map even after both allow-lists are set to ALL.
            options.extend(["-extension_picky", "0"])
    except (OSError, subprocess.SubprocessError):
        # The actual merge will report a precise startup/path error. Keeping
        # the long-established option is the most compatible fallback here.
        pass
    return tuple(options)


async def merge_segments(
    seg_dir: Path,
    output_path: Path,
    segments: list[dict],
    ffmpeg_path: str,
    task=None,
    total_duration: float = 0,
    on_progress=None,
    on_log=None,
) -> None:
    if not segments:
        raise ValueError("没有可合并的分片")

    lease = await acquire_postprocess_lease(
        (seg_dir, output_path),
        task=task,
        waiting_stage="merging",
        waiting_message="正在等待同一磁盘上的其他任务完成合并",
        on_progress=on_progress,
        on_log=on_log,
    )
    try:
        await _merge_segments_unlocked(
            seg_dir=seg_dir,
            output_path=output_path,
            segments=segments,
            ffmpeg_path=ffmpeg_path,
            task=task,
            total_duration=total_duration,
            on_progress=on_progress,
            on_log=on_log,
        )
    finally:
        lease.release()


async def _merge_segments_unlocked(
    seg_dir: Path,
    output_path: Path,
    segments: list[dict],
    ffmpeg_path: str,
    task=None,
    total_duration: float = 0,
    on_progress=None,
    on_log=None,
) -> None:

    merge_inputs: list[Path] = []
    for segment in segments:
        merge_inputs.append(seg_dir / f"{int(segment['index']):06d}.seg")
        if segment.get("init_path"):
            merge_inputs.append(Path(segment["init_path"]))
    estimated_output = estimate_paths_size(merge_inputs)
    await asyncio.to_thread(
        ensure_free_space,
        output_path,
        int(estimated_output * 1.20) + MIN_FREE_RESERVE,
        operation="HLS 合并输出盘",
    )

    use_mpeg_ts_concat = _can_use_mpeg_ts_concat(seg_dir, segments)
    local_playlist = seg_dir.parent / (
        "local-merge.concatf" if use_mpeg_ts_concat else "local-merge.m3u8"
    )
    hls_input_options = () if use_mpeg_ts_concat else await asyncio.to_thread(
        _local_hls_input_options, ffmpeg_path
    )
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

    await asyncio.to_thread(
        write_local_concatf_playlist if use_mpeg_ts_concat else _write_local_hls_playlist,
        local_playlist,
        seg_dir,
        segments,
    )

    if task is not None:
        task.status = TaskStatus.REMUXING
        task.stage = "remuxing"
        task.progress.post_percent = PREPARE_PROGRESS_END
        task.last_log = "ffmpeg 正在转封装"
        _emit_progress(task, on_progress)
    if use_mpeg_ts_concat:
        _emit_log(task, on_log, "检测到 MPEG-TS 分片，使用快速本地拼接")
    _emit_log(
        task,
        on_log,
        f"开始无损转封装：{len(segments)} 个分片，预计时长 {total_duration:.3f} 秒",
    )

    temporary_output = output_path.with_name(
        f"{output_path.stem}.merging{output_path.suffix or '.tmp'}"
    )
    temporary_output.unlink(missing_ok=True)
    duration_args = ["-t", f"{float(total_duration):.6f}"] if total_duration > 0 else []
    input_command = (
        [
            "-protocol_whitelist",
            "file,concatf,concat,crypto,data",
            "-f",
            "mpegts",
            "-i",
            f"concatf:{local_playlist.resolve().as_posix()}",
        ]
        if use_mpeg_ts_concat
        else [
            *hls_input_options,
            "-protocol_whitelist",
            "file,crypto,data",
            "-i",
            str(local_playlist),
        ]
    )
    copy_command = [
        ffmpeg_path,
        "-y",
        "-fflags",
        "+genpts+discardcorrupt",
        *input_command,
        "-c",
        "copy",
        "-avoid_negative_ts",
        "make_zero",
        "-movflags",
        "+faststart",
        *duration_args,
        "-progress",
        "pipe:1",
        "-nostats",
        str(temporary_output),
    ]
    try:
        merge_started = time.monotonic()
        success = await _run_ffmpeg(
            copy_command,
            task=task,
            duration_sec=total_duration,
            on_progress=on_progress,
            **({"on_log": on_log} if on_log is not None else {}),
        )
        copy_verified = False
        if success:
            if task is not None:
                task.stage = "verifying"
                task.progress.post_percent = 99.0
                task.last_log = "正在验证无损转封装输出"
                _emit_progress(task, on_progress)
            try:
                await _verify_output(ffmpeg_path, temporary_output, total_duration)
                copy_verified = True
                _emit_log(
                    task,
                    on_log,
                    f"无损转封装与校验完成，用时 {time.monotonic() - merge_started:.1f} 秒",
                )
            except RuntimeError as exc:
                success = False
                _emit_log(task, on_log, f"无损输出校验未通过：{exc}")
                if task is not None:
                    task.last_log = f"无损输出时间轴异常（{exc}），正在重新编码修复"
                    _emit_progress(task, on_progress)
        if not success:
            temporary_output.unlink(missing_ok=True)
            if task is not None and not task.last_log.startswith("无损输出时间轴异常"):
                task.last_log = "无损转封装失败，正在尝试重新编码"
                _emit_progress(task, on_progress)
            _emit_log(task, on_log, "无损转封装失败，开始兼容重新编码；该阶段会明显慢于无损合并")
            encode_command = [
                ffmpeg_path,
                "-y",
                "-fflags",
                "+genpts+discardcorrupt",
                *input_command,
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
                "-avoid_negative_ts",
                "make_zero",
                "-movflags",
                "+faststart",
                *duration_args,
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
                **({"on_log": on_log} if on_log is not None else {}),
            )
        if not success:
            if task is not None and "ffmpeg" in task.last_log.lower():
                raise RuntimeError(task.last_log)
            raise RuntimeError("ffmpeg 合并失败，未返回可读取的错误信息")

        if not copy_verified:
            if task is not None:
                task.stage = "verifying"
                task.progress.post_percent = 99.0
                task.last_log = "正在验证重新编码输出"
                _emit_progress(task, on_progress)
            await _verify_output(ffmpeg_path, temporary_output, total_duration)
            _emit_log(
                task,
                on_log,
                f"兼容重新编码与校验完成，总用时 {time.monotonic() - merge_started:.1f} 秒",
            )
        # FlushFileBuffers can take seconds on Windows when Defender or a
        # network-backed download directory inspects the new media. Keep the
        # API/event loop responsive while retaining durable atomic publish.
        await asyncio.to_thread(durable_replace, temporary_output, output_path)
    finally:
        temporary_output.unlink(missing_ok=True)

    if task is not None:
        task.progress.post_percent = 100.0
        task.last_log = f"后处理完成: {output_path.name}"
        _emit_progress(task, on_progress)


async def mux_media_tracks(
    *,
    video_path: Path,
    audio_path: Path,
    output_path: Path,
    ffmpeg_path: str,
    task=None,
    total_duration: float = 0,
    on_progress=None,
    on_log=None,
) -> None:
    """Mux independently recorded HLS video/audio tracks into one output."""
    if not video_path.is_file() or video_path.stat().st_size <= 0:
        raise FileNotFoundError("独立视频轨道不存在或为空")
    if not audio_path.is_file() or audio_path.stat().st_size <= 0:
        raise FileNotFoundError("独立音频轨道不存在或为空")

    lease = await acquire_postprocess_lease(
        (video_path, audio_path, output_path),
        task=task,
        waiting_stage="remuxing",
        waiting_message="正在等待同一磁盘上的其他任务完成音视频合并",
        on_progress=on_progress,
        on_log=on_log,
    )
    try:
        await _mux_media_tracks_unlocked(
            video_path=video_path,
            audio_path=audio_path,
            output_path=output_path,
            ffmpeg_path=ffmpeg_path,
            task=task,
            total_duration=total_duration,
            on_progress=on_progress,
            on_log=on_log,
        )
    finally:
        lease.release()


async def _mux_media_tracks_unlocked(
    *,
    video_path: Path,
    audio_path: Path,
    output_path: Path,
    ffmpeg_path: str,
    task=None,
    total_duration: float = 0,
    on_progress=None,
    on_log=None,
) -> None:
    await asyncio.to_thread(
        ensure_free_space,
        output_path,
        video_path.stat().st_size + audio_path.stat().st_size + MIN_FREE_RESERVE,
        operation="音视频合并输出盘",
    )
    video_duration = await _probe_duration(ffmpeg_path, video_path)
    duration_limit = video_duration if video_duration > 0 else float(total_duration or 0)

    temporary = output_path.with_name(
        f"{output_path.stem}.muxing{output_path.suffix or '.mp4'}"
    )
    temporary.unlink(missing_ok=True)
    if task is not None:
        task.status = TaskStatus.REMUXING
        task.stage = "remuxing"
        task.progress.post_percent = PREPARE_PROGRESS_END
        task.last_log = "ffmpeg 正在合并独立视频与音频轨道"
        _emit_progress(task, on_progress)
    _emit_log(task, on_log, "开始无损合并独立视频与音频轨道")
    command = [
        ffmpeg_path,
        "-y",
        "-i",
        str(video_path),
        "-i",
        str(audio_path),
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-c",
        "copy",
        "-avoid_negative_ts",
        "make_zero",
        "-movflags",
        "+faststart",
        "-progress",
        "pipe:1",
        "-nostats",
        *(["-t", f"{duration_limit:.6f}"] if duration_limit > 0 else []),
        str(temporary),
    ]
    try:
        if not await _run_ffmpeg(
            command,
            task=task,
            duration_sec=total_duration,
            on_progress=on_progress,
            **({"on_log": on_log} if on_log is not None else {}),
        ):
            detail = task.last_log if task is not None else ""
            raise RuntimeError(detail or "ffmpeg 无法合并独立视频与音频轨道")
        await _verify_output(ffmpeg_path, temporary, total_duration)
        _emit_log(task, on_log, "独立视频与音频轨道合并并校验完成")
        await asyncio.to_thread(durable_replace, temporary, output_path)
    finally:
        temporary.unlink(missing_ok=True)


async def _start_process(command: list[str]) -> subprocess.Popen[bytes]:
    """Start media tools outside the asyncio thread.

    Windows may synchronously scan a newly extracted executable during
    CreateProcess.  Launching packaged FFmpeg/FFprobe directly from the event
    loop can therefore freeze every API request for several seconds even
    though the child process itself is asynchronous.
    """
    return await asyncio.to_thread(
        subprocess.Popen,
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        **_hidden_subprocess_kwargs(),
    )


async def _terminate_process(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    with contextlib.suppress(ProcessLookupError):
        await asyncio.to_thread(process.terminate)
    try:
        await asyncio.wait_for(asyncio.to_thread(process.wait), timeout=5)
    except asyncio.TimeoutError:
        with contextlib.suppress(ProcessLookupError):
            await asyncio.to_thread(process.kill)
        await asyncio.to_thread(process.wait)


async def _run_ffmpeg(
    command: list[str],
    task=None,
    duration_sec: float = 0,
    on_progress=None,
    on_log=None,
) -> bool:
    process: subprocess.Popen[bytes] | None = None
    stderr_tail = bytearray()
    stderr_task: asyncio.Task | None = None

    async def read_stderr() -> None:
        if process is None or process.stderr is None:
            return
        while True:
            chunk = await asyncio.to_thread(process.stderr.read, 4096)
            if not chunk:
                return
            stderr_tail.extend(chunk)
            if len(stderr_tail) > STDERR_TAIL_LIMIT:
                del stderr_tail[:-STDERR_TAIL_LIMIT]

    try:
        process = await _start_process(command)
        if process.stdout is None or process.stderr is None:
            raise RuntimeError("ffmpeg 标准输出管道创建失败")
        stderr_task = asyncio.create_task(read_stderr())
        while True:
            line = await asyncio.wait_for(
                asyncio.to_thread(process.stdout.readline), timeout=600
            )
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

        return_code = await asyncio.to_thread(process.wait)
        if stderr_task:
            await stderr_task
        if return_code != 0:
            error_text = stderr_tail.decode("utf-8", errors="replace").strip()
            if task is not None:
                task.last_log = f"ffmpeg 失败: {error_text[-500:]}"
                _emit_progress(task, on_progress)
                _emit_log(task, on_log, f"ffmpeg 失败: {error_text[-2000:]}")
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
            _emit_log(task, on_log, task.last_log)
        return False
    except Exception as exc:
        if process is not None:
            await _terminate_process(process)
        if task is not None:
            task.last_log = f"ffmpeg 启动失败: {exc}"
            _emit_progress(task, on_progress)
            _emit_log(task, on_log, task.last_log)
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


def _probe_duration_sync(ffmpeg_path: str, input_file: Path) -> float:
    try:
        completed = subprocess.run(
            [
                _ffprobe_path(ffmpeg_path),
                "-v",
                "quiet",
                "-print_format",
                "json",
                "-show_format",
                str(input_file),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=30,
            **_hidden_subprocess_kwargs(),
        )
        if completed.returncode != 0:
            return 0.0
        data = json.loads(completed.stdout.decode("utf-8", errors="replace"))
        return float(data.get("format", {}).get("duration", 0) or 0)
    except Exception:
        return 0.0


async def _probe_duration(ffmpeg_path: str, input_file: Path) -> float:
    return await asyncio.to_thread(_probe_duration_sync, ffmpeg_path, input_file)


async def _verify_output(
    ffmpeg_path: str,
    output_path: Path,
    expected_duration: float,
) -> None:
    if not output_path.exists() or output_path.stat().st_size == 0:
        raise RuntimeError("输出文件为空或不存在")
    actual_duration = await _probe_duration(ffmpeg_path, output_path)
    if actual_duration <= 0:
        raise RuntimeError("ffprobe 无法读取输出媒体，文件可能损坏")
    if expected_duration >= 3 and actual_duration > 0:
        minimum = expected_duration * 0.9
        if actual_duration < minimum:
            raise RuntimeError(
                f"输出时长异常，期望约 {expected_duration:.1f}s，实际 {actual_duration:.1f}s"
            )
        maximum = expected_duration + max(5.0, expected_duration * 0.1)
        if actual_duration > maximum:
            raise RuntimeError(
                f"输出时长异常，期望约 {expected_duration:.1f}s，实际 {actual_duration:.1f}s"
            )


def _fmt_time(seconds: float) -> str:
    minutes, second = divmod(int(seconds), 60)
    hour, minute = divmod(minutes, 60)
    return f"{hour}:{minute:02d}:{second:02d}"
