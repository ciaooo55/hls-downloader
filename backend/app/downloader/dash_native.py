"""Native MPEG-DASH segment engine.

Downloads the best video and audio representations of a static VOD MPD
with the same segment-level machinery the HLS engine has: shared retry
policy with rate-limit cooldowns, byte-accurate progress, pause that
keeps completed segments, and resume that skips them.  The tracks are
then concatenated and muxed losslessly with ffmpeg.

The engine deliberately reports NativeDashUnsupported before the first
media byte is downloaded whenever the manifest is outside its scope, so
the caller can fall back to the bundled yt-dlp engine with nothing lost.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import re
from copy import deepcopy
from datetime import datetime
from pathlib import Path
from xml.etree import ElementTree

import httpx

from ..config import settings
from ..checksum import verify_task_checksum
from ..models import Task, TaskStatus
from ..network_proxy import policy_httpx_client
from ..request_context import build_task_headers
from ..utils import (
    atomic_write_text,
    durable_replace,
    read_jsonl_prefix,
    sanitize_filename,
    stable_request_key,
    truncate_durable,
)
from .engine import task_output_dir, task_work_dir
from .errors import (
    SharedRetryWindow,
    retry_delay_seconds,
    should_retry_download_error,
    should_share_retry_window,
)
from .merge import _run_ffmpeg, _verify_output
from .disk_space import MIN_FREE_RESERVE, ensure_free_space, estimate_paths_size
from .mpd import NativeDashUnsupported, parse_mpd
from .playback import playback_service, write_playback_plan
from .progress import ProgressTracker
from .throttle import throttle_bytes
from .subtitles import has_cues, merge_webvtt_segments, webvtt_to_srt

MAX_RETRIES = 5
DASH_TIMEOUT = httpx.Timeout(connect=10, read=60, write=30, pool=30)
# The video track lives in the playback service's expected layout
# (segments/*.seg + maps/*.init) so an in-progress download is previewable
# and castable exactly like an HLS task; audio keeps a private directory.
VIDEO_SEG_DIR = "segments"
VIDEO_INIT_NAME = "dash-video.init"
AUDIO_DIR = "a"
LIVE_STATE_FILENAME = "live_state.json"
LIVE_STATE_JOURNAL_FILENAME = "live_state.journal"
LIVE_STATE_JOURNAL_MIN_COMPACT_BYTES = 4 * 1024 * 1024
DASH_VOD_STATE_FILENAME = "dash_vod_segments.json"
DASH_VOD_STATE_VERSION = 1
LIVE_MIN_POLL_SECONDS = 1.0
LIVE_MAX_POLL_SECONDS = 10.0
LIVE_STALL_MIN_SECONDS = 90.0
LIVE_STALL_TARGET_MULTIPLIER = 6.0
_TTML_CLOCK_RE = re.compile(r"^(?:(\d+):)?(\d{2}):(\d{2}(?:\.\d+)?)$")
_TTML_SECONDS_RE = re.compile(r"^(\d+(?:\.\d+)?)s$")


def _ttml_seconds(value: str) -> float | None:
    raw = str(value or "").strip()
    clock = _TTML_CLOCK_RE.fullmatch(raw)
    if clock:
        return (
            int(clock.group(1) or 0) * 3600
            + int(clock.group(2)) * 60
            + float(clock.group(3))
        )
    seconds = _TTML_SECONDS_RE.fullmatch(raw)
    return float(seconds.group(1)) if seconds else None


def _ttml_clock(value: float) -> str:
    bounded = max(0.0, float(value))
    hours, remainder = divmod(bounded, 3600)
    minutes, seconds = divmod(remainder, 60)
    return f"{int(hours):02d}:{int(minutes):02d}:{seconds:06.3f}"


def _merge_segmented_ttml(files: list[Path], segments: list[dict], destination: Path) -> None:
    documents = [ElementTree.parse(path) for path in files]
    cue_groups = [
        [node for node in document.getroot().iter() if node.tag.rsplit("}", 1)[-1] == "p"]
        for document in documents
    ]
    root = documents[0].getroot()
    target_div = next(
        (node for node in root.iter() if node.tag.rsplit("}", 1)[-1] == "div"),
        None,
    )
    if target_div is None:
        raise RuntimeError("TTML 字幕缺少 div 容器")
    # Rebuild the cue list in segment order. Styling/layout from the first
    # document remains authoritative; individual cue attributes are retained.
    for node in list(target_div):
        if node.tag.rsplit("}", 1)[-1] == "p":
            target_div.remove(node)
    for index, cues in enumerate(cue_groups):
        segment = segments[index] if index < len(segments) else {}
        start = float(segment.get("start") or 0)
        duration = float(segment.get("duration") or 0)
        times = [
            parsed
            for cue in cues
            for name in ("begin", "end")
            if (parsed := _ttml_seconds(cue.get(name, ""))) is not None
        ]
        relative = bool(start > 0 and times and max(times) <= duration + 0.5)
        for cue in cues:
            copied = deepcopy(cue)
            if relative:
                for name in ("begin", "end"):
                    parsed = _ttml_seconds(copied.get(name, ""))
                    if parsed is not None:
                        copied.set(name, _ttml_clock(parsed + start))
            target_div.append(copied)
    destination.parent.mkdir(parents=True, exist_ok=True)
    ElementTree.ElementTree(root).write(
        destination,
        encoding="utf-8",
        xml_declaration=True,
    )


def _is_webm_track(track: dict) -> bool:
    mime = str(track.get("mime") or "").lower()
    codecs = str(track.get("codecs") or "").lower()
    return "webm" in mime or codecs.startswith(("vp8", "vp09", "vp9", "vp0"))


class NativeDashEngine:
    def __init__(self, task: Task, on_progress=None, on_log=None) -> None:
        self.task = task
        self.on_progress = on_progress or (lambda task: None)
        self.on_log = on_log or (lambda task_id, message: None)
        self._retry_window = SharedRetryWindow()
        self.tracker = ProgressTracker()
        self._header_cache: dict[str, dict[str, str]] = {}
        self._live_checkpoint_tracks: dict[str, dict] | None = None
        self._vod_resume_records: dict[str, dict[str, int | str]] = {}
        self._vod_resume_lock = asyncio.Lock()

    def _publish(self) -> None:
        self.on_progress(self.task)

    def _log(self, message: str) -> None:
        self.on_log(self.task.id, message)

    def _set_stage(self, stage: str, message: str) -> None:
        self.task.stage = stage
        self.task.last_log = message
        self.on_log(self.task.id, f"[{stage}] {message}")
        self._publish()

    def _is_canceled(self) -> bool:
        return bool(self.task.cancel_event and self.task.cancel_event.is_set())

    def _refresh_playback_progress(self) -> None:
        try:
            snapshot = playback_service.snapshot(
                self.task.id,
                self.task.status.value,
                self.task.output_path,
            )
        except Exception:
            return
        progress = self.task.progress
        progress.playable_segments = snapshot.available_segments
        progress.playable_duration = snapshot.available_duration
        progress.media_duration = snapshot.total_duration

    def _is_pausing(self) -> bool:
        return bool(self.task.pause_event and self.task.pause_event.is_set())

    @staticmethod
    def _vod_job_identity(kind: str, slot: str, track: dict, url: str) -> str:
        descriptor = {
            "kind": kind,
            "slot": slot,
            "representation": str(track.get("id") or ""),
            "resource": stable_request_key(url, ignore_host=True),
            "mime": str(track.get("mime") or ""),
            "codecs": str(track.get("codecs") or ""),
        }
        encoded = json.dumps(
            descriptor,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
        return hashlib.sha256(encoded).hexdigest()

    @staticmethod
    def _vod_state_path(task_dir: Path) -> Path:
        return task_dir / DASH_VOD_STATE_FILENAME

    def _write_vod_state(self, task_dir: Path) -> None:
        atomic_write_text(
            self._vod_state_path(task_dir),
            json.dumps(
                {
                    "version": DASH_VOD_STATE_VERSION,
                    "files": self._vod_resume_records,
                },
                sort_keys=True,
            ),
        )

    def _prepare_vod_resume(self, task_dir: Path, jobs: list[dict]) -> None:
        try:
            payload = json.loads(
                self._vod_state_path(task_dir).read_text(encoding="utf-8")
            )
        except (OSError, ValueError):
            payload = {}
        saved = (
            payload.get("files")
            if payload.get("version") == DASH_VOD_STATE_VERSION
            else {}
        )
        if not isinstance(saved, dict):
            saved = {}
        valid: dict[str, dict[str, int | str]] = {}
        expected: set[str] = set()
        for job in jobs:
            destination = Path(job["destination"])
            relative = destination.relative_to(task_dir).as_posix()
            expected.add(relative)
            record = saved.get(relative)
            try:
                size = destination.stat().st_size
            except OSError:
                size = 0
            try:
                expected_size = int((record or {}).get("size") or 0)
            except (AttributeError, TypeError, ValueError):
                expected_size = 0
            if (
                size > 0
                and expected_size == size
                and isinstance(record, dict)
                and record.get("identity") == job["identity"]
            ):
                valid[relative] = {
                    "identity": str(job["identity"]),
                    "size": size,
                }
            else:
                destination.unlink(missing_ok=True)
            destination.with_name(destination.name + ".tmp").unlink(missing_ok=True)

        candidates = [
            task_dir / "maps" / VIDEO_INIT_NAME,
            task_dir / AUDIO_DIR / "init.mp4",
            *(task_dir / VIDEO_SEG_DIR).glob("*.seg"),
            *(task_dir / AUDIO_DIR).glob("*.m4s"),
        ]
        for path in candidates:
            try:
                relative = path.relative_to(task_dir).as_posix()
            except ValueError:
                continue
            if relative not in expected:
                path.unlink(missing_ok=True)

        self._vod_resume_records = valid
        self._write_vod_state(task_dir)

    async def _checkpoint_vod_job(
        self,
        task_dir: Path,
        destination: Path,
        identity: str,
    ) -> None:
        size = destination.stat().st_size
        if size <= 0:
            return
        relative = destination.relative_to(task_dir).as_posix()
        async with self._vod_resume_lock:
            self._vod_resume_records[relative] = {
                "identity": identity,
                "size": size,
            }
            await asyncio.to_thread(self._write_vod_state, task_dir)

    def _headers(self, request_url: str = "") -> dict[str, str]:
        """Per-URL headers so manifest-origin cookies never leak to CDNs.

        build_task_headers scopes cookies/authorization by request origin;
        memoized per origin because a stream has thousands of same-origin
        segment requests.
        """
        origin = ""
        if request_url:
            try:
                parsed = httpx.URL(request_url)
                origin = f"{parsed.scheme}://{parsed.host}:{parsed.port or ''}"
            except Exception:
                origin = request_url
        cached = self._header_cache.get(origin)
        if cached is None:
            cached = build_task_headers(self.task, request_url=request_url)
            self._header_cache[origin] = cached
        return cached

    async def _request_control(
        self,
        client: httpx.AsyncClient,
        url: str,
        *,
        stage: str,
        label: str,
    ) -> httpx.Response:
        """Fetch a DASH manifest through the task-wide transient retry gate."""
        last_error: Exception | None = None
        for attempt in range(MAX_RETRIES):
            if self._is_canceled() or self._is_pausing():
                raise asyncio.CancelledError
            if not await self._retry_window.wait(
                lambda: self._is_canceled() or self._is_pausing()
            ):
                raise asyncio.CancelledError
            try:
                response = await client.get(url, headers=self._headers(url))
                response.raise_for_status()
                if self.task.progress.connection_status == "rate_limited":
                    self.task.progress.connection_status = "running"
                    self._set_stage(stage, f"{label}限流结束，继续请求")
                return response
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                last_error = exc
                if not should_retry_download_error(exc) or attempt >= MAX_RETRIES - 1:
                    break
                delay = retry_delay_seconds(exc, min(2**attempt, 10))
                self.task.progress.reconnect_count += 1
                if should_share_retry_window(exc):
                    remaining, extended = await self._retry_window.extend(delay)
                    if extended:
                        self.task.progress.connection_status = "rate_limited"
                        self._set_stage(
                            stage,
                            f"源站暂时限流，{label}共同等待约 {max(1, int(remaining + 0.999))} 秒",
                        )
                else:
                    self.task.progress.connection_status = "reconnecting"
                    self._log(
                        f"[{label}] 第 {attempt + 1}/{MAX_RETRIES} 次失败，"
                        f"{delay:g} 秒后重试: {exc}"
                    )
                    await asyncio.sleep(delay)
        if last_error is not None:
            raise last_error
        raise RuntimeError(f"{label}请求失败")

    async def run(self) -> bool:
        """Download, concat and mux the manifest's best tracks.

        Returns True when the task reached a terminal or paused state here.
        Raises NativeDashUnsupported (before any media download) when the
        manifest needs the fallback engine.
        """
        task = self.task
        task_dir = task_work_dir(task)
        task_dir.mkdir(parents=True, exist_ok=True)
        async with policy_httpx_client(
            follow_redirects=True,
            timeout=DASH_TIMEOUT,
            deny_private_networks=bool(task.engine_state.get("browser_originated")),
        ) as client:
            saved_state = self._load_live_state(task_dir)
            has_recorded = any(
                (track or {}).get("segments")
                for track in ((saved_state or {}).get("tracks") or {}).values()
            )
            try:
                response = await self._request_control(
                    client,
                    task.url,
                    stage="parsing",
                    label="DASH 清单",
                )
                text = response.text
                if "<MPD" not in text[:4096]:
                    raise NativeDashUnsupported("清单不是 MPD 格式")
            except asyncio.CancelledError:
                raise
            except NativeDashUnsupported:
                raise
            except Exception:
                # A finished live event usually takes its manifest offline.
                # The captured segments are the only copy that will ever
                # exist, so merge them instead of failing forever.
                if not has_recorded:
                    raise
                self._log("[recording] 直播清单已不可用，直接合并已录制的内容")
                return await self._finalize_offline(task_dir, saved_state or {})
            manifest_url = str(response.url or task.url)
            parsed = parse_mpd(
                manifest_url,
                text,
                preferred_video=task.selected_video,
                preferred_audio=task.selected_audio,
            )
            tracks = {
                kind: parsed[kind]
                for kind in ("video", "audio")
                if parsed.get(kind)
            }
            if not tracks:
                raise NativeDashUnsupported("MPD 中没有可下载轨道")
            atomic_write_text(task_dir / "manifest.mpd", text)

            # A resumed recording stays in recording mode even after the
            # stream flipped to static VOD: its captured segments start
            # mid-timeline and must never be reused as VOD positions.
            if parsed["type"] == "dynamic" or has_recorded:
                task.engine_state["live"] = True
                return await self._record_live(client, parsed, manifest_url, task_dir)
            # A retried task whose stream has since ended downloads as VOD.
            task.engine_state.pop("live", None)

            total_segments = sum(
                len(track["segments"]) + (1 if track["init_url"] else 0)
                for track in tracks.values()
            )
            duration = float(parsed.get("duration") or 0)
            task.progress.total_segments = total_segments
            task.progress.completed_segments = 0
            task.progress.media_duration = duration
            task.progress.max_workers = min(max(1, task.concurrency), 8)
            task.progress.connection_status = "running"
            task.status = TaskStatus.DOWNLOADING_SEGMENTS
            quality = ""
            video = tracks.get("video")
            if video and video.get("height"):
                quality = f"{video['height']}p "
            self._set_stage(
                "downloading_segments",
                f"原生 DASH 引擎：{quality}{len(tracks)} 条轨道，共 {total_segments} 个分片",
            )

            jobs: list[dict] = []
            video = tracks.get("video")
            if video:
                seg_dir = task_dir / VIDEO_SEG_DIR
                seg_dir.mkdir(parents=True, exist_ok=True)
                init_path: Path | None = None
                if video["init_url"]:
                    (task_dir / "maps").mkdir(parents=True, exist_ok=True)
                    init_path = task_dir / "maps" / VIDEO_INIT_NAME
                    jobs.append({
                        "destination": init_path,
                        "url": video["init_url"],
                        "identity": self._vod_job_identity(
                            "video", "init", video, video["init_url"]
                        ),
                    })
                for index, segment in enumerate(video["segments"]):
                    destination = seg_dir / f"{index:06d}.seg"
                    jobs.append({
                        "destination": destination,
                        "url": segment["url"],
                        "identity": self._vod_job_identity(
                            "video", str(index), video, segment["url"]
                        ),
                    })
                if (
                    init_path is not None
                    and not video.get("single_file")
                    and not _is_webm_track(video)
                ):
                    # Full plan up front: the playback service serves the
                    # contiguous prefix of files as they land, so preview
                    # works while the download runs — same as HLS.
                    await asyncio.to_thread(
                        write_playback_plan,
                        task_dir,
                        [
                            {
                                "index": index,
                                "duration": float(segment.get("duration") or 0),
                                "discontinuity": False,
                                "init_path": str(init_path),
                            }
                            for index, segment in enumerate(video["segments"])
                        ],
                        duration,
                    )
            audio = tracks.get("audio")
            if audio:
                track_dir = task_dir / AUDIO_DIR
                track_dir.mkdir(parents=True, exist_ok=True)
                if audio["init_url"]:
                    jobs.append({
                        "destination": track_dir / "init.mp4",
                        "url": audio["init_url"],
                        "identity": self._vod_job_identity(
                            "audio", "init", audio, audio["init_url"]
                        ),
                    })
                for index, segment in enumerate(audio["segments"]):
                    jobs.append({
                        "destination": track_dir / f"{index:06d}.m4s",
                        "url": segment["url"],
                        "identity": self._vod_job_identity(
                            "audio", str(index), audio, segment["url"]
                        ),
                    })

            await asyncio.to_thread(self._prepare_vod_resume, task_dir, jobs)

            semaphore = asyncio.Semaphore(task.progress.max_workers)
            self.tracker.start(total_segments)
            stopped = False

            async def fetch(destination: Path, url: str, identity: str) -> None:
                nonlocal stopped
                async with semaphore:
                    if stopped or self._is_canceled() or self._is_pausing():
                        stopped = True
                        return
                    task.progress.active_workers += 1
                    task.progress.active_slots += 1
                    try:
                        await self._download_one(client, url, destination)
                        if not destination.exists():
                            stopped = True
                            return
                        await self._checkpoint_vod_job(
                            task_dir, destination, identity
                        )
                        self.tracker.add_completed(destination.stat().st_size)
                        snapshot = self.tracker.snapshot()
                        task.progress.completed_segments = snapshot["completed"]
                        task.progress.downloaded_bytes = snapshot["downloaded_bytes"]
                        task.progress.speed_bytes_per_sec = snapshot["speed"]
                        task.progress.eta_seconds = snapshot["eta"]
                        if total_segments:
                            task.progress.progress_percent = (
                                snapshot["completed"] * 100 / total_segments
                            )
                        if destination.suffix == ".seg":
                            self._refresh_playback_progress()
                    finally:
                        task.progress.active_workers -= 1
                        task.progress.active_slots -= 1
                        self._publish()

            results = await asyncio.gather(
                *(
                    fetch(job["destination"], job["url"], job["identity"])
                    for job in jobs
                ),
                return_exceptions=True,
            )
            if self._is_canceled():
                raise asyncio.CancelledError
            error = next(
                (item for item in results if isinstance(item, Exception)), None
            )
            if error is not None:
                raise error
            if stopped or self._is_pausing():
                if task.pause_event is not None:
                    task.pause_event.clear()
                task.status = TaskStatus.PAUSED
                task.progress.connection_status = "idle"
                self._set_stage("paused", "已暂停，已完成的分片会在继续时复用")
                return True

        finalized = await self._finalize_tracks(
            task_dir,
            tracks,
            {kind: len(track["segments"]) for kind, track in tracks.items()},
            duration,
            starts={
                kind: float((track["segments"][0] or {}).get("start") or 0)
                for kind, track in tracks.items()
                if track["segments"]
            },
            cleanup=False,
        )
        if finalized and task.status is TaskStatus.DONE:
            await self._download_dash_subtitles(parsed.get("subtitle_tracks") or [])
        if not settings.keep_temp_files:
            import shutil

            await asyncio.to_thread(shutil.rmtree, task_dir, True)
        return finalized

    async def _finalize_tracks(
        self,
        task_dir: Path,
        tracks: dict,
        counts: dict[str, int],
        duration: float,
        starts: dict[str, float] | None = None,
        cleanup: bool = True,
    ) -> bool:
        """Concat each downloaded track and mux them into the output file.

        starts carries each track's first-segment position on the media
        timeline; live tracks routinely begin a segment apart, so the mux
        offsets them instead of stacking both at zero (A/V desync).
        """
        task = self.task
        task.status = TaskStatus.MERGING
        task.progress.post_percent = 5.0
        self._set_stage("merging", "正在拼接 DASH 轨道")
        track_files: list[tuple[str, Path]] = []
        for kind, track in tracks.items():
            joined = task_dir / f"{kind}.track.mp4"
            if kind == "video":
                seg_dir = task_dir / VIDEO_SEG_DIR
                init_path = (
                    task_dir / "maps" / VIDEO_INIT_NAME if track["init_url"] else None
                )
                extension = ".seg"
            else:
                seg_dir = task_dir / AUDIO_DIR
                init_path = (
                    task_dir / AUDIO_DIR / "init.mp4" if track["init_url"] else None
                )
                extension = ".m4s"
            await asyncio.to_thread(
                self._concat_track,
                seg_dir,
                init_path,
                counts[kind],
                extension,
                joined,
            )
            track_files.append((kind, joined))

        # VP8 (and some VP9) streams are WebM-only; Matroska accepts any
        # codec losslessly, so it is the safe container for those.
        container = ".mkv" if any(
            "webm" in str(track.get("mime") or "").lower()
            or str(track.get("codecs") or "").lower().startswith("vp0")
            or str(track.get("codecs") or "").lower().startswith("vp8")
            for track in tracks.values()
        ) else ".mp4"
        output = self._reserve_output(task, container)
        task.engine_state["reserved_output_path"] = str(output)
        await asyncio.to_thread(
            ensure_free_space,
            output,
            estimate_paths_size(path for _kind, path in track_files) + MIN_FREE_RESERVE,
            operation="DASH 合并输出盘",
        )
        task.status = TaskStatus.REMUXING
        self._set_stage("remuxing", "正在无损合并音视频轨")
        temporary = output.with_name(f"{output.stem}.merging{output.suffix}")
        temporary.unlink(missing_ok=True)
        command = [settings.ffmpeg_path, "-y", "-hide_banner", "-loglevel", "error"]
        offsets = starts or {}
        base_start = min((offsets.get(kind, 0.0) for kind, _ in track_files), default=0.0)
        for kind, path in track_files:
            offset = offsets.get(kind, 0.0) - base_start
            # -itsoffset is an input option: it must precede its own -i.
            if offset > 0.001:
                command += ["-itsoffset", f"{offset:.3f}"]
            command += ["-i", str(path)]
        command += ["-c", "copy"]
        if container == ".mp4":
            command += ["-movflags", "+faststart"]
        command += ["-progress", "pipe:1", str(temporary)]
        try:
            success = await _run_ffmpeg(
                command, task=task, duration_sec=duration, on_progress=self.on_progress
            )
            if not success or not temporary.exists() or temporary.stat().st_size <= 0:
                raise RuntimeError("ffmpeg 合并 DASH 轨道失败")
            if duration > 0:
                await _verify_output(settings.ffmpeg_path, temporary, duration)
            await asyncio.to_thread(durable_replace, temporary, output)
        except BaseException:
            # Never leave a zero-byte reservation or half-written file in
            # the user's download directory on failure/cancel/shutdown.
            temporary.unlink(missing_ok=True)
            if output.exists() and output.stat().st_size == 0:
                output.unlink(missing_ok=True)
            raise
        task.engine_state.pop("reserved_output_path", None)

        task.filename = output.name
        task.output_path = str(output)
        task.engine_state["output_is_file"] = True
        task.engine_state["stream_path"] = str(output)
        size = output.stat().st_size
        task.engine_state["total_size"] = size
        task.progress.downloaded_bytes = max(task.progress.downloaded_bytes, size)
        task.progress.total_bytes = task.progress.downloaded_bytes
        task.progress.progress_percent = 100.0
        task.progress.post_percent = 100.0
        task.progress.connection_status = "idle"
        if not await verify_task_checksum(
            task, output, on_progress=self.on_progress, on_log=self.on_log
        ):
            return True
        task.status = TaskStatus.DONE
        task.finished_at = datetime.now().isoformat()
        self._set_stage("done", f"完成: {output.name} ({size / 1048576:.1f} MB)")
        if cleanup and not settings.keep_temp_files:
            import shutil

            await asyncio.to_thread(shutil.rmtree, task_dir, True)
        return True

    async def _download_dash_subtitles(self, tracks: list[dict]) -> None:
        if not tracks or not getattr(settings, "download_subtitles", True):
            return
        if not self.task.output_path:
            return
        output = Path(self.task.output_path)
        base = output.with_suffix("")
        used: set[str] = set()
        saved = 0
        async with policy_httpx_client(
            follow_redirects=True,
            timeout=DASH_TIMEOUT,
            deny_private_networks=bool(self.task.engine_state.get("browser_originated")),
        ) as client:
            for position, track in enumerate(tracks, 1):
                raw_label = str(
                    track.get("lang") or track.get("name") or track.get("id") or f"sub{position}"
                )
                label = sanitize_filename(raw_label).strip(". ") or f"sub{position}"
                candidate = label
                suffix = 2
                while candidate.lower() in used:
                    candidate = f"{label}.{suffix}"
                    suffix += 1
                label = candidate
                used.add(label.lower())
                try:
                    vtt_path = await self._download_dash_subtitle_track(
                        client, track, base, label
                    )
                    if vtt_path is None:
                        continue
                    saved += 1
                    self._log(f"[subtitles] 已保存 DASH 字幕: {vtt_path.name}")
                except asyncio.CancelledError:
                    raise
                except Exception as exc:
                    self._log(f"[subtitles] DASH 字幕 {label} 下载失败: {exc}")
        if saved:
            self._log(f"[subtitles] 共保存 {saved} 条 DASH 字幕轨道")

    async def _download_dash_subtitle_track(
        self,
        client: httpx.AsyncClient,
        track: dict,
        base: Path,
        label: str,
    ) -> Path | None:
        task_dir = task_work_dir(self.task) / "dash-subtitles" / label
        task_dir.mkdir(parents=True, exist_ok=True)
        files: list[Path] = []
        if track.get("init_url"):
            init_path = task_dir / "init.bin"
            await self._download_one(client, str(track["init_url"]), init_path)
            files.append(init_path)
        for index, segment in enumerate(track.get("segments") or []):
            destination = task_dir / f"{index:06d}.bin"
            await self._download_one(client, str(segment["url"]), destination)
            files.append(destination)
        if not files:
            return None

        mime = str(track.get("mime") or "").lower()
        codecs = str(track.get("codecs") or "").lower()
        vtt_path = base.with_name(f"{base.name}.{label}.vtt")
        if mime.startswith("text/vtt") or (not track.get("init_url") and codecs.startswith("wvtt")):
            texts = [path.read_text(encoding="utf-8-sig", errors="replace") for path in files]
            merged = merge_webvtt_segments(texts)
            if not has_cues(merged):
                return None
            vtt_path.write_text(merged, encoding="utf-8")
        elif mime == "application/ttml+xml":
            ttml_path = base.with_name(f"{base.name}.{label}.ttml")
            if len(files) == 1:
                ttml_path.write_bytes(files[0].read_bytes())
            else:
                await asyncio.to_thread(
                    _merge_segmented_ttml,
                    files,
                    list(track.get("segments") or []),
                    ttml_path,
                )
            return ttml_path
        elif codecs.startswith(("wvtt", "stpp")) or mime == "application/mp4":
            joined = task_dir / "subtitle.mp4"
            with joined.open("wb") as stream:
                for path in files:
                    stream.write(path.read_bytes())
            success = await _run_ffmpeg(
                [
                    settings.ffmpeg_path,
                    "-y",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-i",
                    str(joined),
                    str(vtt_path),
                ],
                task=self.task,
                duration_sec=0,
                on_progress=self.on_progress,
            )
            if not success or not vtt_path.is_file():
                raise RuntimeError("FFmpeg 无法转换 fMP4 DASH 字幕")
        else:
            self._log(
                f"[subtitles] 跳过暂不支持的 DASH 字幕格式: {mime or codecs or 'unknown'}"
            )
            return None

        merged = vtt_path.read_text(encoding="utf-8-sig", errors="replace")
        if has_cues(merged):
            vtt_path.with_suffix(".srt").write_text(
                webvtt_to_srt(merged), encoding="utf-8"
            )
        return vtt_path

    @staticmethod
    def _track_layout(task_dir: Path, kind: str) -> tuple[Path, str]:
        if kind == "video":
            return task_dir / VIDEO_SEG_DIR, ".seg"
        return task_dir / AUDIO_DIR, ".m4s"

    @staticmethod
    def _track_init_path(task_dir: Path, kind: str) -> Path:
        if kind == "video":
            return task_dir / "maps" / VIDEO_INIT_NAME
        return task_dir / AUDIO_DIR / "init.mp4"

    @staticmethod
    def _live_track_fingerprint(kind: str, track: dict) -> str:
        descriptor = {
            "kind": kind,
            "representation": str(track.get("id") or ""),
            "mime": str(track.get("mime") or ""),
            "codecs": str(track.get("codecs") or ""),
            "init": stable_request_key(
                str(track.get("init_url") or ""), ignore_host=True
            ),
            "single_file": bool(track.get("single_file")),
        }
        return hashlib.sha256(
            json.dumps(descriptor, sort_keys=True, separators=(",", ":")).encode(
                "utf-8"
            )
        ).hexdigest()

    @staticmethod
    def _live_state_journal_path(task_dir: Path) -> Path:
        return task_dir / LIVE_STATE_JOURNAL_FILENAME

    def _read_live_state(self, task_dir: Path) -> dict | None:
        path = task_dir / LIVE_STATE_FILENAME
        journal = self._live_state_journal_path(task_dir)
        if not path.exists() and not journal.exists():
            return None
        payload: dict = {"version": 3, "tracks": {}}
        if path.exists():
            try:
                loaded = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, ValueError):
                return None
            if not isinstance(loaded, dict) or not isinstance(loaded.get("tracks"), dict):
                return None
            payload = loaded

        def apply_track_delta(kind: str, delta: dict) -> None:
            if delta.get("deleted"):
                payload.setdefault("tracks", {}).pop(kind, None)
                return
            track = payload.setdefault("tracks", {}).setdefault(kind, {"segments": []})
            by_index = {
                int(item["index"]): item
                for item in track.get("segments", [])
                if isinstance(item, dict) and str(item.get("index", "")).lstrip("-").isdigit()
            }
            for raw_index in delta.get("remove", []):
                try:
                    by_index.pop(int(raw_index), None)
                except (TypeError, ValueError):
                    continue
            for item in delta.get("upsert", []):
                if not isinstance(item, dict):
                    continue
                try:
                    by_index[int(item["index"])] = item
                except (KeyError, TypeError, ValueError):
                    continue
            metadata = delta.get("metadata")
            if isinstance(metadata, dict):
                track.update(metadata)
            track["segments"] = [by_index[index] for index in sorted(by_index)]

        if journal.exists():
            try:
                records, journal_size = read_jsonl_prefix(journal)
                accepted_offset = 0
                for event, end_offset in records:
                    deltas = event.get("tracks") if event.get("version") == 1 else None
                    if not isinstance(deltas, dict):
                        break
                    for kind, delta in deltas.items():
                        if isinstance(delta, dict):
                            apply_track_delta(str(kind), delta)
                    accepted_offset = end_offset
                if accepted_offset < journal_size:
                    truncate_durable(journal, accepted_offset)
            except OSError:
                return None
        payload["version"] = 3
        return payload

    def _load_live_state(self, task_dir: Path) -> dict | None:
        payload = self._read_live_state(task_dir)
        self._live_checkpoint_tracks = (
            {
                str(kind): {
                    "segments": {
                        int(item["index"]): item
                        for item in track.get("segments", [])
                        if isinstance(item, dict)
                        and str(item.get("index", "")).lstrip("-").isdigit()
                    },
                    "metadata": {
                        key: value for key, value in track.items() if key != "segments"
                    },
                }
                for kind, track in payload.get("tracks", {}).items()
                if isinstance(track, dict)
            }
            if payload is not None
            else None
        )
        return payload

    def _save_live_state(
        self,
        task_dir: Path,
        state: dict[str, dict],
        *,
        force_compact: bool = False,
        changed_tracks: dict[str, list[dict]] | None = None,
    ) -> None:
        def metadata(value: dict) -> dict:
            return {
                "fingerprint": str(value.get("fingerprint") or ""),
                "mime": str(value.get("mime") or ""),
                "codecs": str(value.get("codecs") or ""),
                "has_init": bool(value.get("has_init")),
                "init_size": int(value.get("init_size") or 0),
            }

        destination = task_dir / LIVE_STATE_FILENAME
        journal = self._live_state_journal_path(task_dir)
        previous_tracks = self._live_checkpoint_tracks
        if previous_tracks is None:
            previous = self._read_live_state(task_dir)
            previous_tracks = {
                str(kind): {
                    "segments": {
                        int(item["index"]): item
                        for item in track.get("segments", [])
                        if isinstance(item, dict)
                        and str(item.get("index", "")).lstrip("-").isdigit()
                    },
                    "metadata": {
                        key: value for key, value in track.items() if key != "segments"
                    },
                }
                for kind, track in (previous or {}).get("tracks", {}).items()
                if isinstance(track, dict)
            }
        next_tracks = {
            kind: {
                "segments": dict(track.get("segments", {})),
                "metadata": dict(track.get("metadata", {})),
            }
            for kind, track in previous_tracks.items()
        }
        deltas: dict[str, dict] = {}
        for kind, value in state.items():
            old_track = previous_tracks.get(kind, {"segments": {}, "metadata": {}})
            old = old_track.get("segments", {})
            new_metadata = metadata(value)
            if changed_tracks is None:
                new = {int(item["index"]): dict(item) for item in value["entries"]}
                removed = sorted(set(old) - set(new))
                upserts = [new[index] for index in sorted(new) if old.get(index) != new[index]]
            else:
                new = dict(old)
                upserts = []
                for item in changed_tracks.get(kind, []):
                    copied = dict(item)
                    index = int(copied["index"])
                    if new.get(index) != copied:
                        new[index] = copied
                        upserts.append(copied)
                removed = []
            next_tracks[kind] = {"segments": new, "metadata": new_metadata}
            if upserts or removed or new_metadata != old_track.get("metadata", {}):
                deltas[kind] = {
                    "remove": removed,
                    "upsert": upserts,
                    "metadata": new_metadata,
                }
        if changed_tracks is None:
            for kind in set(previous_tracks) - set(state):
                old_track = previous_tracks.get(kind, {"segments": {}})
                next_tracks.pop(kind, None)
                deltas[kind] = {
                    "remove": sorted(old_track.get("segments", {})),
                    "upsert": [],
                    "metadata": {},
                    "deleted": True,
                }

        def payload() -> dict:
            return {
                "version": 3,
                "tracks": {
                    kind: {
                        "segments": [
                            track["segments"][index]
                            for index in sorted(track["segments"])
                        ],
                        **track["metadata"],
                    }
                    for kind, track in next_tracks.items()
                },
            }

        if force_compact or not destination.exists():
            atomic_write_text(destination, json.dumps(payload(), ensure_ascii=False))
            journal.unlink(missing_ok=True)
        elif deltas:
            line = json.dumps(
                {"version": 1, "tracks": deltas},
                ensure_ascii=False,
                separators=(",", ":"),
            ) + "\n"
            journal.parent.mkdir(parents=True, exist_ok=True)
            with journal.open("a", encoding="utf-8", newline="\n") as stream:
                stream.write(line)
                stream.flush()
                os.fsync(stream.fileno())
            if journal.stat().st_size >= max(
                LIVE_STATE_JOURNAL_MIN_COMPACT_BYTES,
                destination.stat().st_size * 2,
            ):
                atomic_write_text(destination, json.dumps(payload(), ensure_ascii=False))
                journal.unlink(missing_ok=True)
        self._live_checkpoint_tracks = next_tracks

    def _pause_exit(self) -> bool:
        task = self.task
        if task.pause_event is not None:
            task.pause_event.clear()
        task.status = TaskStatus.PAUSED
        task.progress.connection_status = "idle"
        self._set_stage("paused", "已停止，尚未录制到内容，可重新开始")
        return True

    async def _finalize_offline(self, task_dir: Path, saved_state: dict) -> bool:
        """Merge a recording whose live manifest is gone.

        Track metadata comes from manifest.mpd (always written before the
        first segment), and only the validated contiguous file prefix is
        merged — exactly the rule the resume path uses.
        """
        task = self.task
        task.engine_state["live"] = True
        manifest_path = task_dir / "manifest.mpd"
        tracks: dict = {}
        if manifest_path.exists():
            try:
                parsed = parse_mpd(
                    task.url,
                    manifest_path.read_text(encoding="utf-8"),
                    preferred_video=task.selected_video,
                    preferred_audio=task.selected_audio,
                )
                tracks = {
                    kind: parsed[kind]
                    for kind in ("video", "audio")
                    if parsed.get(kind)
                }
            except Exception:
                tracks = {}
        counts: dict[str, int] = {}
        starts: dict[str, float] = {}
        duration = 0.0
        for kind in ("video", "audio"):
            saved_track = ((saved_state.get("tracks") or {}).get(kind) or {})
            entries = saved_track.get("segments") or []
            if not entries:
                continue
            seg_dir, extension = self._track_layout(task_dir, kind)
            kept = 0
            total = 0.0
            for item in entries:
                try:
                    index = int(item["index"])
                except (KeyError, TypeError, ValueError):
                    break
                path = seg_dir / f"{index:06d}{extension}"
                if index != kept or not path.exists() or path.stat().st_size == 0:
                    break
                expected_size = int(item.get("size") or 0)
                if expected_size and path.stat().st_size != expected_size:
                    break
                if kept == 0:
                    starts[kind] = float(item.get("start") or 0)
                kept += 1
                total += float(item.get("duration") or 0)
            if not kept:
                continue
            counts[kind] = kept
            if kind not in tracks:
                # Without the manifest, assume the standard fMP4 layout the
                # recorder itself wrote (init + media segments).
                tracks[kind] = {
                    "init_url": str(self._track_init_path(task_dir, kind))
                    if self._track_init_path(task_dir, kind).exists() else "",
                    "mime": "video/mp4" if kind == "video" else "audio/mp4",
                    "codecs": "",
                    "segments": [],
                }
            if saved_track:
                # The live manifest may now describe a different
                # representation.  Checkpoint metadata belongs to the bytes
                # on disk and therefore wins for final container selection.
                tracks[kind] = dict(tracks[kind])
                tracks[kind]["mime"] = str(
                    saved_track.get("mime") or tracks[kind].get("mime") or ""
                )
                tracks[kind]["codecs"] = str(
                    saved_track.get("codecs") or tracks[kind].get("codecs") or ""
                )
                if "has_init" in saved_track:
                    tracks[kind]["init_url"] = (
                        "checkpoint-init" if saved_track.get("has_init") else ""
                    )
            if kind == "video" or not duration:
                duration = max(duration, total)
        if not counts:
            raise RuntimeError("直播源已不可用，且没有可合并的已录制分片")
        task.progress.total_segments = sum(counts.values())
        task.progress.completed_segments = task.progress.total_segments
        task.progress.media_duration = duration
        return await self._finalize_tracks(
            task_dir,
            {kind: tracks[kind] for kind in counts},
            counts,
            duration,
            starts=starts,
        )

    async def _record_live(
        self,
        client: httpx.AsyncClient,
        parsed: dict,
        manifest_url: str,
        task_dir: Path,
    ) -> bool:
        """Record a dynamic MPD until stopped, stalled or ended.

        Mirrors the HLS live recorder: segments append per manifest refresh
        (deduplicated by timeline identity), state persists for crash
        recovery, the stop request finalizes and muxes what was captured.
        """
        task = self.task
        tracks = {
            kind: parsed[kind] for kind in ("video", "audio") if parsed.get(kind)
        }
        segment_basis = max(
            [seg["duration"] for track in tracks.values() for seg in track["segments"][:1]]
            or [2.0]
        )
        poll_seconds = min(
            LIVE_MAX_POLL_SECONDS,
            max(LIVE_MIN_POLL_SECONDS, float(parsed.get("update_period") or segment_basis)),
        )
        stall_window = max(
            LIVE_STALL_MIN_SECONDS,
            LIVE_STALL_TARGET_MULTIPLIER * max(segment_basis, poll_seconds),
        )
        max_minutes = max(0, int(getattr(settings, "live_record_max_minutes", 0) or 0))
        max_seconds = max_minutes * 60.0

        saved = self._load_live_state(task_dir) or {}
        saved_tracks = saved.get("tracks") or {}
        incompatible = []
        for kind, track in tracks.items():
            saved_track = saved_tracks.get(kind) or {}
            if not saved_track.get("segments") or not saved_track.get("fingerprint"):
                continue
            if saved_track["fingerprint"] != self._live_track_fingerprint(kind, track):
                incompatible.append(kind)
        if incompatible:
            self._log(
                "[recording] 检测到 DASH 轨道/编码器已变化，先安全合并上次录制"
            )
            return await self._finalize_offline(task_dir, saved)

        state: dict[str, dict] = {}
        self.tracker.start(0)
        for kind, track in tracks.items():
            seg_dir, extension = self._track_layout(task_dir, kind)
            seg_dir.mkdir(parents=True, exist_ok=True)
            kept: list[dict] = []
            for item in (saved.get("tracks", {}).get(kind) or {}).get("segments", []):
                try:
                    index = int(item["index"])
                    identity = int(item["identity"])
                except (KeyError, TypeError, ValueError):
                    break
                path = seg_dir / f"{index:06d}{extension}"
                if index != len(kept) or not path.exists() or path.stat().st_size == 0:
                    break
                expected_size = int(item.get("size") or 0)
                if expected_size and path.stat().st_size != expected_size:
                    break
                kept.append({
                    "index": index,
                    "identity": identity,
                    "duration": float(item.get("duration") or 0),
                    "start": float(item.get("start") or 0),
                    "size": expected_size or path.stat().st_size,
                })
                self.tracker.add_completed(path.stat().st_size)
            # Files past the persisted contiguous prefix belong to a crashed
            # batch whose indexes will be reassigned; never splice them in.
            for stray in seg_dir.glob(f"*{extension}"):
                try:
                    stray_index = int(stray.stem)
                except ValueError:
                    continue
                if stray_index >= len(kept):
                    stray.unlink(missing_ok=True)
            state[kind] = {
                "entries": kept,
                "total_duration": sum(entry["duration"] for entry in kept),
                "last_identity": max((entry["identity"] for entry in kept), default=-1),
                "fingerprint": self._live_track_fingerprint(kind, track),
                "mime": str(track.get("mime") or ""),
                "codecs": str(track.get("codecs") or ""),
                "has_init": bool(track.get("init_url")),
                "init_size": 0,
            }
        if any(value["entries"] for value in state.values()):
            self._log(
                "[recording] 继续上次录制："
                + "、".join(
                    f"{kind} {len(value['entries'])} 片"
                    for kind, value in state.items()
                )
            )

        for kind, track in tracks.items():
            if not track["init_url"]:
                continue
            init_path = self._track_init_path(task_dir, kind)
            init_path.parent.mkdir(parents=True, exist_ok=True)
            saved_track = saved_tracks.get(kind) or {}
            expected_init_size = int(saved_track.get("init_size") or 0)
            if init_path.exists() and (
                not saved_track.get("segments")
                or (expected_init_size and init_path.stat().st_size != expected_init_size)
            ):
                init_path.unlink(missing_ok=True)
            if not init_path.exists():
                await self._download_one(client, track["init_url"], init_path)
                if not init_path.exists():
                    return self._pause_exit()
            state[kind]["init_size"] = init_path.stat().st_size

        video_state = state.get("video")
        # An audio-only recording still needs a duration for the cap, the UI
        # and the finalize log.
        duration_state = video_state or state.get("audio")
        video_track = tracks.get("video")
        plan_init = (
            str(self._track_init_path(task_dir, "video"))
            if video_track and video_track["init_url"] and not _is_webm_track(video_track)
            else ""
        )

        def write_live_plan(
            changed_entries: list[dict] | None = None,
            *,
            force_compact: bool = False,
        ) -> None:
            if not video_state or not plan_init:
                return
            entries = video_state["entries"] if changed_entries is None else changed_entries
            plan_segments = [
                {
                    "index": entry["index"],
                    "duration": entry["duration"],
                    "discontinuity": False,
                    "init_path": plan_init,
                }
                for entry in entries
            ]
            write_playback_plan(
                task_dir,
                plan_segments,
                float(video_state["total_duration"]),
                force_compact=force_compact,
                changed_segments=(plan_segments if changed_entries is not None else None),
            )

        def refresh_counters() -> None:
            total = sum(len(value["entries"]) for value in state.values())
            task.progress.total_segments = total
            task.progress.completed_segments = total
            snapshot = self.tracker.snapshot()
            task.progress.downloaded_bytes = snapshot["downloaded_bytes"]
            task.progress.speed_bytes_per_sec = snapshot["speed"]
            if duration_state:
                task.progress.media_duration = float(duration_state["total_duration"])
            self._publish()

        task.status = TaskStatus.DOWNLOADING_SEGMENTS
        task.progress.max_workers = 1
        task.progress.connection_status = "running"
        write_live_plan()
        refresh_counters()
        self._refresh_playback_progress()
        self._set_stage("recording", "DASH 直播录制中，停止录制后自动合并")

        loop = asyncio.get_running_loop()
        last_new_segment = loop.time()
        current = tracks
        finish_reason = ""

        async def append_fresh(current_tracks: dict) -> bool:
            nonlocal finish_reason
            appended = False
            for kind, track in current_tracks.items():
                if kind not in state:
                    continue
                track_state = state[kind]
                seg_dir, extension = self._track_layout(task_dir, kind)
                for segment in track["segments"]:
                    identity = int(segment.get("identity") or 0)
                    if identity <= track_state["last_identity"]:
                        continue
                    if self._is_canceled():
                        raise asyncio.CancelledError
                    if self._is_pausing():
                        finish_reason = "已停止录制"
                        return appended
                    index = len(track_state["entries"])
                    destination = seg_dir / f"{index:06d}{extension}"
                    try:
                        await self._download_one(client, segment["url"], destination)
                    except asyncio.CancelledError:
                        raise
                    except Exception as exc:
                        # Do not advance the committed cursor on a failed
                        # request. Signed DASH URLs can expire between the
                        # manifest poll and the segment GET; the next poll may
                        # expose the same timeline identity with a refreshed
                        # URL. Advancing here permanently loses that segment
                        # and can leave a recording at 0 seconds until stall.
                        # Once the live window slides past it, the identity is
                        # naturally no longer present and recording proceeds.
                        task.progress.failed_segments += 1
                        self._log(f"[recording] {kind} 分片下载失败，将在后续清单重试: {exc}")
                        continue
                    if not destination.exists():
                        finish_reason = "已停止录制"
                        return appended
                    track_state["entries"].append({
                        "index": index,
                        "identity": identity,
                        "duration": float(segment.get("duration") or 0),
                        "start": float(segment.get("start") or 0),
                        "size": destination.stat().st_size,
                    })
                    track_state["total_duration"] += float(segment.get("duration") or 0)
                    track_state["last_identity"] = identity
                    self.tracker.add_completed(destination.stat().st_size)
                    appended = True
                    if kind == "video":
                        await asyncio.to_thread(
                            write_live_plan,
                            [track_state["entries"][-1]],
                        )
                        self._refresh_playback_progress()
                    await asyncio.to_thread(
                        self._save_live_state,
                        task_dir,
                        state,
                        changed_tracks={kind: [track_state["entries"][-1]]},
                    )
                    refresh_counters()
            return appended

        while True:
            if self._is_canceled():
                raise asyncio.CancelledError
            if self._is_pausing():
                finish_reason = "已停止录制"
                break

            appended = await append_fresh(current)
            if finish_reason:
                break
            recorded_any = any(value["entries"] for value in state.values())
            video_duration = float(duration_state["total_duration"]) if duration_state else 0.0
            if appended:
                last_new_segment = loop.time()
                self._set_stage(
                    "recording",
                    f"DASH 直播录制中：已录制 {video_duration:.0f} 秒"
                    f"（{task.progress.total_segments} 分片）",
                )
            if max_seconds and video_duration >= max_seconds:
                finish_reason = f"已达到录制时长上限 {max_minutes} 分钟"
                break
            if loop.time() - last_new_segment > stall_window:
                if recorded_any:
                    finish_reason = "直播源已停止更新，自动结束录制"
                    break
                raise RuntimeError("直播清单长时间没有新分片，直播源可能已停止")

            deadline = loop.time() + poll_seconds
            while loop.time() < deadline:
                if self._is_canceled() or self._is_pausing():
                    break
                await asyncio.sleep(0.2)
            if self._is_canceled():
                raise asyncio.CancelledError
            if self._is_pausing():
                finish_reason = "已停止录制"
                break

            try:
                response = await self._request_control(
                    client,
                    manifest_url,
                    stage="recording",
                    label="DASH 直播清单",
                )
                refreshed = parse_mpd(
                    str(response.url or manifest_url),
                    response.text,
                    preferred_video=task.selected_video,
                    preferred_audio=task.selected_audio,
                )
                manifest_url = str(response.url or manifest_url)
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                if any(value["entries"] for value in state.values()):
                    self._log(f"[recording] 直播清单刷新失败，结束录制: {exc}")
                    finish_reason = "直播清单已不可用，录制结束"
                    break
                raise
            current = {
                kind: refreshed[kind]
                for kind in ("video", "audio")
                if refreshed.get(kind) and kind in state
            }
            changed = [kind for kind in state if kind not in current] + [
                kind
                for kind, track in current.items()
                if self._live_track_fingerprint(kind, track)
                != state[kind]["fingerprint"]
            ]
            if changed:
                self._log(
                    "[recording] DASH 直播轨道/编码器发生变化，结束当前文件以避免混流"
                )
                finish_reason = "直播轨道已切换，当前录制安全结束"
                break
            if refreshed["type"] != "dynamic":
                await append_fresh(current)
                finish_reason = finish_reason or "直播已结束"
                break

        counts = {
            kind: len(value["entries"])
            for kind, value in state.items()
            if value["entries"]
        }
        if not counts:
            return self._pause_exit()
        await asyncio.to_thread(write_live_plan, force_compact=True)
        await asyncio.to_thread(
            self._save_live_state,
            task_dir,
            state,
            force_compact=True,
        )
        final_duration = (
            float(video_state["total_duration"])
            if video_state and video_state["entries"]
            else max(
                float(value["total_duration"])
                for value in state.values()
                if value["entries"]
            )
        )
        task.progress.media_duration = final_duration
        self._set_stage(
            "recording",
            f"{finish_reason}，共录制 {final_duration:.0f} 秒，正在合并",
        )
        return await self._finalize_tracks(
            task_dir,
            {kind: tracks[kind] for kind in counts},
            counts,
            final_duration,
            starts={
                kind: float(state[kind]["entries"][0].get("start") or 0)
                for kind in counts
            },
        )

    async def _download_one(
        self,
        client: httpx.AsyncClient,
        url: str,
        destination: Path,
    ) -> None:
        task = self.task
        headers = self._headers(url)
        if destination.exists() and destination.stat().st_size > 0:
            # Byte accounting happens in the caller via the tracker.
            return
        last_error: Exception | None = None
        for attempt in range(MAX_RETRIES):
            if self._is_canceled() or self._is_pausing():
                return
            if not await self._retry_window.wait(
                lambda: self._is_canceled() or self._is_pausing()
            ):
                return
            temporary = destination.with_name(destination.name + ".tmp")
            try:
                received = 0
                async with client.stream("GET", url, headers=headers) as response:
                    response.raise_for_status()
                    with temporary.open("wb") as stream:
                        async for chunk in response.aiter_bytes(256 * 1024):
                            if self._is_canceled() or self._is_pausing():
                                raise asyncio.CancelledError
                            await throttle_bytes(len(chunk), task)
                            stream.write(chunk)
                            received += len(chunk)
                if received <= 0:
                    raise RuntimeError("分片响应为空")
                await asyncio.to_thread(durable_replace, temporary, destination)
                return
            except asyncio.CancelledError:
                temporary.unlink(missing_ok=True)
                if self._is_pausing() and not self._is_canceled():
                    return
                raise
            except Exception as exc:
                temporary.unlink(missing_ok=True)
                last_error = exc
                task.progress.reconnect_count += 1
                task.progress.connection_status = "reconnecting"
                if not should_retry_download_error(exc):
                    break
                if attempt < MAX_RETRIES - 1:
                    delay = retry_delay_seconds(exc, min(2**attempt, 10))
                    if should_share_retry_window(exc):
                        remaining, extended = await self._retry_window.extend(delay)
                        if extended:
                            self._set_stage(
                                "downloading_segments",
                                f"源站限流，共同等待 {max(1, int(remaining))} 秒",
                            )
                    else:
                        await asyncio.sleep(delay)
        if last_error is not None:
            raise last_error

    @staticmethod
    def _concat_track(
        seg_dir: Path,
        init_path: Path | None,
        count: int,
        extension: str,
        destination: Path,
    ) -> None:
        """Join init + media segments into one continuous fMP4 track file."""
        with destination.open("wb") as output:
            if init_path is not None and init_path.exists():
                output.write(init_path.read_bytes())
            for index in range(count):
                segment_path = seg_dir / f"{index:06d}{extension}"
                with segment_path.open("rb") as source:
                    while True:
                        block = source.read(1024 * 1024)
                        if not block:
                            break
                        output.write(block)

    @staticmethod
    def _reserve_output(task: Task, container: str = ".mp4") -> Path:
        from ..utils import sanitize_filename

        name = sanitize_filename(task.filename or task.title or task.id)
        if Path(name).suffix.lower() in {".mpd", ".xml"}:
            name = Path(name).stem or task.id
        # Dotted display names ("Show.S01E02.1080p") are not containers;
        # ffmpeg picks its muxer from the extension, so always force one.
        if not name.lower().endswith(container):
            name += container
        directory = task_output_dir(task)
        directory.mkdir(parents=True, exist_ok=True)
        base = directory / name
        for index in range(10000):
            candidate = base if index == 0 else base.with_name(
                f"{base.stem}_{index}{base.suffix}"
            )
            try:
                # Atomically claim the name so two same-named tasks can
                # never write the same output file concurrently.
                candidate.open("xb").close()
                return candidate
            except FileExistsError:
                continue
        raise RuntimeError(f"无法分配输出名称: {base.name}")
