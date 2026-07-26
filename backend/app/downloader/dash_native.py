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
import json
import shutil
from datetime import datetime
from pathlib import Path

import httpx

from ..config import settings
from ..checksum import verify_task_checksum
from ..models import Task, TaskStatus
from ..request_context import build_task_headers
from .engine import task_output_dir, task_work_dir
from .errors import (
    SharedRetryWindow,
    retry_delay_seconds,
    should_retry_download_error,
    should_share_retry_window,
)
from .merge import _run_ffmpeg, _verify_output
from .mpd import NativeDashUnsupported, parse_mpd
from .playback import playback_service, write_playback_plan
from .progress import ProgressTracker
from .throttle import throttle_bytes

MAX_RETRIES = 5
DASH_TIMEOUT = httpx.Timeout(connect=10, read=60, write=30, pool=30)
# The video track lives in the playback service's expected layout
# (segments/*.seg + maps/*.init) so an in-progress download is previewable
# and castable exactly like an HLS task; audio keeps a private directory.
VIDEO_SEG_DIR = "segments"
VIDEO_INIT_NAME = "dash-video.init"
AUDIO_DIR = "a"
LIVE_STATE_FILENAME = "live_state.json"
LIVE_MIN_POLL_SECONDS = 1.0
LIVE_MAX_POLL_SECONDS = 10.0
LIVE_STALL_MIN_SECONDS = 90.0
LIVE_STALL_TARGET_MULTIPLIER = 6.0


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

    async def run(self) -> bool:
        """Download, concat and mux the manifest's best tracks.

        Returns True when the task reached a terminal or paused state here.
        Raises NativeDashUnsupported (before any media download) when the
        manifest needs the fallback engine.
        """
        task = self.task
        task_dir = task_work_dir(task)
        task_dir.mkdir(parents=True, exist_ok=True)
        async with httpx.AsyncClient(
            follow_redirects=True, timeout=DASH_TIMEOUT
        ) as client:
            response = await client.get(task.url, headers=self._headers(task.url))
            response.raise_for_status()
            text = response.text
            if "<MPD" not in text[:4096]:
                raise NativeDashUnsupported("清单不是 MPD 格式")
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
            (task_dir / "manifest.mpd").write_text(text, encoding="utf-8")

            if parsed["type"] == "dynamic":
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

            jobs: list[tuple[Path, str]] = []
            video = tracks.get("video")
            if video:
                seg_dir = task_dir / VIDEO_SEG_DIR
                seg_dir.mkdir(parents=True, exist_ok=True)
                init_path: Path | None = None
                if video["init_url"]:
                    (task_dir / "maps").mkdir(parents=True, exist_ok=True)
                    init_path = task_dir / "maps" / VIDEO_INIT_NAME
                    jobs.append((init_path, video["init_url"]))
                for index, segment in enumerate(video["segments"]):
                    jobs.append((seg_dir / f"{index:06d}.seg", segment["url"]))
                if (
                    init_path is not None
                    and not video.get("single_file")
                    and not _is_webm_track(video)
                ):
                    # Full plan up front: the playback service serves the
                    # contiguous prefix of files as they land, so preview
                    # works while the download runs — same as HLS.
                    write_playback_plan(
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
                    jobs.append((track_dir / "init.mp4", audio["init_url"]))
                for index, segment in enumerate(audio["segments"]):
                    jobs.append((track_dir / f"{index:06d}.m4s", segment["url"]))

            semaphore = asyncio.Semaphore(task.progress.max_workers)
            self.tracker.start(total_segments)
            stopped = False

            async def fetch(destination: Path, url: str) -> None:
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
                *(fetch(path, url) for path, url in jobs), return_exceptions=True
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

        return await self._finalize_tracks(
            task_dir,
            tracks,
            {kind: len(track["segments"]) for kind, track in tracks.items()},
            duration,
        )

    async def _finalize_tracks(
        self,
        task_dir: Path,
        tracks: dict,
        counts: dict[str, int],
        duration: float,
    ) -> bool:
        """Concat each downloaded track and mux them into the output file."""
        task = self.task
        task.status = TaskStatus.MERGING
        task.progress.post_percent = 5.0
        self._set_stage("merging", "正在拼接 DASH 轨道")
        track_files: list[Path] = []
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
            track_files.append(joined)

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
        task.status = TaskStatus.REMUXING
        self._set_stage("remuxing", "正在无损合并音视频轨")
        temporary = output.with_name(f"{output.stem}.merging{output.suffix}")
        temporary.unlink(missing_ok=True)
        command = [settings.ffmpeg_path, "-y", "-hide_banner", "-loglevel", "error"]
        for path in track_files:
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
            temporary.replace(output)
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
        if not settings.keep_temp_files:
            import shutil

            await asyncio.to_thread(shutil.rmtree, task_dir, True)
        return True

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

    def _load_live_state(self, task_dir: Path) -> dict | None:
        path = task_dir / LIVE_STATE_FILENAME
        if not path.exists():
            return None
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            return None
        return payload if isinstance(payload, dict) else None

    def _save_live_state(self, task_dir: Path, state: dict[str, dict]) -> None:
        payload = {
            "version": 1,
            "tracks": {
                kind: {"segments": value["entries"]}
                for kind, value in state.items()
            },
        }
        destination = task_dir / LIVE_STATE_FILENAME
        temporary = destination.with_name(destination.name + ".tmp")
        try:
            temporary.write_text(
                json.dumps(payload, ensure_ascii=False), encoding="utf-8"
            )
            temporary.replace(destination)
        finally:
            temporary.unlink(missing_ok=True)

    def _pause_exit(self) -> bool:
        task = self.task
        if task.pause_event is not None:
            task.pause_event.clear()
        task.status = TaskStatus.PAUSED
        task.progress.connection_status = "idle"
        self._set_stage("paused", "已停止，尚未录制到内容，可重新开始")
        return True

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
                kept.append({
                    "index": index,
                    "identity": identity,
                    "duration": float(item.get("duration") or 0),
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
                "last_identity": max((entry["identity"] for entry in kept), default=-1),
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
            if not init_path.exists():
                await self._download_one(client, track["init_url"], init_path)
                if not init_path.exists():
                    return self._pause_exit()

        video_state = state.get("video")
        video_track = tracks.get("video")
        plan_init = (
            str(self._track_init_path(task_dir, "video"))
            if video_track and video_track["init_url"] and not _is_webm_track(video_track)
            else ""
        )

        def write_live_plan() -> None:
            if not video_state or not plan_init:
                return
            entries = video_state["entries"]
            write_playback_plan(
                task_dir,
                [
                    {
                        "index": entry["index"],
                        "duration": entry["duration"],
                        "discontinuity": False,
                        "init_path": plan_init,
                    }
                    for entry in entries
                ],
                sum(entry["duration"] for entry in entries),
            )

        def refresh_counters() -> None:
            total = sum(len(value["entries"]) for value in state.values())
            task.progress.total_segments = total
            task.progress.completed_segments = total
            snapshot = self.tracker.snapshot()
            task.progress.downloaded_bytes = snapshot["downloaded_bytes"]
            task.progress.speed_bytes_per_sec = snapshot["speed"]
            if video_state:
                task.progress.media_duration = sum(
                    entry["duration"] for entry in video_state["entries"]
                )
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
                        # The live window slides on; skip a dead segment
                        # rather than aborting the whole recording.
                        task.progress.failed_segments += 1
                        self._log(f"[recording] {kind} 分片下载失败已跳过: {exc}")
                        track_state["last_identity"] = identity
                        continue
                    if not destination.exists():
                        finish_reason = "已停止录制"
                        return appended
                    track_state["entries"].append({
                        "index": index,
                        "identity": identity,
                        "duration": float(segment.get("duration") or 0),
                    })
                    track_state["last_identity"] = identity
                    self.tracker.add_completed(destination.stat().st_size)
                    appended = True
                    if kind == "video":
                        write_live_plan()
                        self._refresh_playback_progress()
                    self._save_live_state(task_dir, state)
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
            video_duration = (
                sum(entry["duration"] for entry in video_state["entries"])
                if video_state
                else 0.0
            )
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
                response = await client.get(
                    manifest_url, headers=self._headers(manifest_url)
                )
                response.raise_for_status()
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
        self._save_live_state(task_dir, state)
        final_duration = (
            sum(entry["duration"] for entry in video_state["entries"])
            if video_state and video_state["entries"]
            else max(
                sum(entry["duration"] for entry in value["entries"])
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
                temporary.replace(destination)
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
