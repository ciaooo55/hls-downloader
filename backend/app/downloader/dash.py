from __future__ import annotations

import asyncio
import shutil
from datetime import datetime
from pathlib import Path

from ..config import settings
from ..checksum import verify_task_checksum
from ..models import Task, TaskStatus
from ..utils import sanitize_filename
from ..request_context import build_task_headers
from .engine import SeeklessEngine, publish_path, task_output_dir, task_work_dir
from .errors import diagnose_download_error, format_download_error


class _StopDownload(Exception):
    pass


class DashDownloader(SeeklessEngine):
    def __init__(self, task: Task, on_progress=None, on_log=None) -> None:
        self.task = task
        self.on_progress = on_progress or (lambda task: None)
        self.on_log = on_log or (lambda task_id, message: None)

    def _publish(self) -> None:
        self.on_progress(self.task)

    def _set_stage(self, stage: str, message: str) -> None:
        self.task.stage = stage
        self.task.last_log = message
        self.on_log(self.task.id, f"[{stage}] {message}")
        self._publish()

    def _stopping(self) -> bool:
        return bool(
            (self.task.cancel_event and self.task.cancel_event.is_set())
            or (self.task.pause_event and self.task.pause_event.is_set())
        )

    async def run(self) -> None:
        task = self.task
        task_dir = task_work_dir(task)
        task_dir.mkdir(parents=True, exist_ok=True)
        try:
            task.status = TaskStatus.DOWNLOADING
            task.started_at = task.started_at or datetime.now().isoformat()
            task.progress.connection_status = "connecting"
            self._set_stage("parsing", "正在解析 DASH 清单")
            result = await asyncio.to_thread(self._run_ytdlp, task_dir)
            if self._stopping():
                if task.cancel_event and task.cancel_event.is_set():
                    task.status = TaskStatus.CANCELED
                    task.finished_at = datetime.now().isoformat()
                    self._set_stage("canceled", "已取消")
                    if not settings.keep_temp_files:
                        await asyncio.to_thread(shutil.rmtree, task_dir, True)
                else:
                    task.status = TaskStatus.PAUSED
                    self._set_stage("paused", "已暂停，可继续下载")
                return
            output = Path(result)
            if not output.exists() or output.stat().st_size <= 0:
                raise RuntimeError("DASH 下载没有生成有效文件")
            final_name = sanitize_filename(task.filename or output.name)
            if not Path(final_name).suffix:
                final_name += output.suffix or ".mp4"
            destination = self._unique_path(task_output_dir(task) / final_name)
            await asyncio.to_thread(publish_path, output, destination)
            task.filename = destination.name
            task.output_path = str(destination)
            task.engine_state["output_is_file"] = True
            task.engine_state["stream_path"] = str(destination)
            task.engine_state["total_size"] = destination.stat().st_size
            task.progress.downloaded_bytes = destination.stat().st_size
            task.progress.total_bytes = task.progress.downloaded_bytes
            task.progress.progress_percent = 100.0
            task.progress.post_percent = 100.0
            task.progress.connection_status = "idle"
            if not await verify_task_checksum(task, destination, on_progress=self.on_progress, on_log=self.on_log):
                return
            task.status = TaskStatus.DONE
            task.finished_at = datetime.now().isoformat()
            self._set_stage("done", f"完成: {destination.name}")
            if not settings.keep_temp_files:
                await asyncio.to_thread(shutil.rmtree, task_dir, True)
        except asyncio.CancelledError:
            task.progress.connection_status = "idle"
            task.status = TaskStatus.PAUSED if not (task.cancel_event and task.cancel_event.is_set()) else TaskStatus.CANCELED
            task.stage = "interrupted" if task.status is TaskStatus.PAUSED else "canceled"
            task.last_log = "程序已关闭，DASH 临时文件已保留" if task.status is TaskStatus.PAUSED else "已取消"
            self._publish()
            raise
        except _StopDownload:
            if task.cancel_event and task.cancel_event.is_set():
                task.status = TaskStatus.CANCELED
                task.finished_at = datetime.now().isoformat()
                self._set_stage("canceled", "已取消")
                if not settings.keep_temp_files:
                    await asyncio.to_thread(shutil.rmtree, task_dir, True)
            else:
                task.status = TaskStatus.PAUSED
                self._set_stage("paused", "已暂停，可继续下载")
        except Exception as exc:
            details = diagnose_download_error(exc, stage=task.stage, url=task.url, task_context=task)
            if "drm" in str(exc).lower() or "protected" in str(exc).lower():
                details = details.__class__(
                    code="DASH_DRM_UNSUPPORTED",
                    message="该 DASH 使用 DRM 保护",
                    hint="受保护媒体不能下载，请使用网站官方离线功能。",
                    stage=task.stage,
                    url=details.url,
                )
                task.status = TaskStatus.UNSUPPORTED
            else:
                task.status = TaskStatus.FAILED
            task.error_code = details.code
            task.error_stage = details.stage
            task.error_url = details.url
            task.error_hint = details.hint
            task.error_message = format_download_error(details)
            task.finished_at = datetime.now().isoformat()
            task.progress.connection_status = "error"
            self._set_stage(task.status.value, task.error_message)

    def _run_ytdlp(self, task_dir: Path) -> str:
        try:
            import yt_dlp
        except ImportError as exc:
            raise RuntimeError("DASH 下载组件 yt-dlp 未安装") from exc

        task = self.task
        before = {path.resolve() for path in task_dir.glob("*") if path.is_file()}

        def progress_hook(data: dict) -> None:
            if self._stopping():
                raise _StopDownload
            status = data.get("status")
            if status == "downloading":
                task.status = TaskStatus.DOWNLOADING
                task.stage = "downloading"
                task.progress.connection_status = "running"
                task.progress.downloaded_bytes = int(data.get("downloaded_bytes") or 0)
                task.progress.total_bytes = int(data.get("total_bytes") or data.get("total_bytes_estimate") or 0)
                task.progress.speed_bytes_per_sec = float(data.get("speed") or 0)
                task.progress.eta_seconds = float(data.get("eta") or 0)
                if task.progress.total_bytes:
                    task.progress.progress_percent = min(
                        100.0,
                        task.progress.downloaded_bytes * 100 / task.progress.total_bytes,
                    )
                task.last_log = f"DASH 下载 {task.progress.progress_percent:.1f}%"
                self._publish()
            elif status == "finished":
                task.status = TaskStatus.REMUXING
                task.stage = "remuxing"
                task.progress.post_percent = 95.0
                task.last_log = "正在合并 DASH 音视频轨"
                self._publish()

        headers = build_task_headers(task)
        options = {
            "outtmpl": str(task_dir / "payload.%(ext)s"),
            "format": "bestvideo+bestaudio/best",
            "merge_output_format": "mp4",
            "continuedl": True,
            "nopart": False,
            "quiet": True,
            "no_warnings": True,
            "http_headers": headers,
            "ffmpeg_location": str(Path(settings.ffmpeg_path).parent),
            "progress_hooks": [progress_hook],
            "noplaylist": True,
        }
        with yt_dlp.YoutubeDL(options) as downloader:
            info = downloader.extract_info(task.url, download=True)
            prepared = Path(downloader.prepare_filename(info))
        candidates = [path for path in task_dir.glob("payload.*") if path.is_file() and path.suffix not in {".part", ".ytdl"}]
        if prepared.exists():
            return str(prepared)
        new_files = [path for path in candidates if path.resolve() not in before]
        if not new_files:
            raise RuntimeError("yt-dlp 未返回 DASH 输出文件")
        return str(max(new_files, key=lambda path: path.stat().st_size))

    @staticmethod
    def _unique_path(path: Path) -> Path:
        path.parent.mkdir(parents=True, exist_ok=True)
        for index in range(10000):
            candidate = path if index == 0 else path.with_name(f"{path.stem}_{index}{path.suffix}")
            if not candidate.exists():
                return candidate
        raise RuntimeError(f"无法分配输出名称: {path.name}")
