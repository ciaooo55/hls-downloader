from __future__ import annotations

import asyncio
import json
import shutil
import threading
from datetime import datetime
from pathlib import Path

import httpx

from ..config import settings
from ..models import Task, TaskStatus
from ..utils import sanitize_filename
from .errors import diagnose_download_error, format_download_error
from .engine import publish_path, task_output_dir, task_work_dir


_SESSION_LOCK = threading.Lock()
_SHARED_SESSION = None
_RESUME_ALERT_LOCK = asyncio.Lock()


def _torrent_session(lt):
    global _SHARED_SESSION
    with _SESSION_LOCK:
        created = _SHARED_SESSION is None
        if _SHARED_SESSION is None:
            _SHARED_SESSION = lt.session()
        session = _SHARED_SESSION
        per_torrent = max(50, int(settings.bt_max_connections))
        session_settings = {
            "connections_limit": per_torrent * max(1, int(settings.max_concurrent_tasks)),
            "connection_speed": 50,
            "upload_rate_limit": int(settings.bt_upload_limit_kib) * 1024,
            "download_rate_limit": 0,
            "active_downloads": max(1, int(settings.max_concurrent_tasks)),
            "active_limit": max(2, int(settings.max_concurrent_tasks) + 1),
            "enable_dht": bool(settings.bt_enable_dht),
            "enable_lsd": True,
            "enable_upnp": True,
            "enable_natpmp": True,
            "enable_incoming_tcp": True,
            "enable_outgoing_tcp": True,
            "enable_incoming_utp": True,
            "enable_outgoing_utp": True,
            "announce_to_all_trackers": True,
            "announce_to_all_tiers": True,
            "alert_mask": int(lt.alert.category_t.error_notification | lt.alert.category_t.storage_notification),
        }
        if created:
            session_settings["listen_interfaces"] = "0.0.0.0:0"
        session.apply_settings(session_settings)
        return session


class TorrentDownloader:
    def __init__(self, task: Task, on_progress=None, on_log=None) -> None:
        self.task = task
        self.on_progress = on_progress or (lambda task: None)
        self.on_log = on_log or (lambda task_id, message: None)
        self._handle = None
        self._session = None
        self._priority_piece: int | None = None
        self._piece_length = 0
        self._stream_file_offset = 0
        self._stream_path: Path | None = None

    def _publish(self) -> None:
        self.on_progress(self.task)

    def _set_stage(self, stage: str, message: str) -> None:
        self.task.stage = stage
        self.task.last_log = message
        self.on_log(self.task.id, f"[{stage}] {message}")
        self._publish()

    def request_seek(self, value: int) -> None:
        if self._piece_length > 0:
            self._priority_piece = max(
                0,
                (self._stream_file_offset + int(value)) // self._piece_length,
            )
        else:
            self._priority_piece = max(0, int(value))
        self._prioritize_piece(self._priority_piece)

    def _prioritize_piece(self, piece: int) -> None:
        handle = self._handle
        if handle is None or not handle.is_valid():
            return
        try:
            for offset in range(12):
                handle.set_piece_deadline(piece + offset, offset * 250)
        except Exception:
            return

    async def wait_for_range(self, start: int, end: int, timeout: float = 45.0) -> Path:
        if self._handle is None or self._stream_path is None or self._piece_length <= 0:
            raise FileNotFoundError("BT 播放文件尚未准备好")
        first = (self._stream_file_offset + start) // self._piece_length
        last = (self._stream_file_offset + max(start, end)) // self._piece_length
        for order, piece in enumerate(range(first, last + 1)):
            try:
                self._handle.set_piece_deadline(piece, order * 200)
            except Exception:
                pass
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            if all(self._handle.have_piece(piece) for piece in range(first, last + 1)):
                if self._stream_path.exists():
                    return self._stream_path
            if asyncio.get_running_loop().time() >= deadline:
                raise TimeoutError("目标 BT piece 尚未下载完成")
            await asyncio.sleep(0.1)

    def select_files(self, indexes: list[int]) -> None:
        self.task.engine_state["selected_files"] = sorted({int(value) for value in indexes if int(value) >= 0})
        if self._handle is not None and self._handle.is_valid():
            self._apply_file_priorities()

    def _is_canceled(self) -> bool:
        return bool(self.task.cancel_event and self.task.cancel_event.is_set())

    def _is_pausing(self) -> bool:
        return bool(self.task.pause_event and self.task.pause_event.is_set())

    @staticmethod
    def _load_libtorrent():
        try:
            import libtorrent as lt
        except ImportError as exc:
            raise RuntimeError("BT 下载组件 libtorrent 未安装") from exc
        return lt

    async def run(self) -> None:
        task = self.task
        task_dir = task_work_dir(task)
        payload_dir = task_dir / "payload"
        resume_path = task_dir / "torrent.fastresume"
        torrent_path = task_dir / "source.torrent"
        payload_dir.mkdir(parents=True, exist_ok=True)
        try:
            lt = self._load_libtorrent()
            task.started_at = task.started_at or datetime.now().isoformat()
            task.status = TaskStatus.FETCHING_METADATA
            task.progress.connection_status = "connecting"
            self._set_stage("fetching_metadata", "正在获取 BT 元数据")
            session = _torrent_session(lt)
            self._session = session
            params = None
            if resume_path.exists():
                try:
                    params = lt.read_resume_data(resume_path.read_bytes())
                    params.save_path = str(payload_dir)
                except Exception:
                    resume_path.unlink(missing_ok=True)
                    params = None
            if params is None and task.url.startswith("magnet:"):
                params = lt.parse_magnet_uri(task.url)
                params.save_path = str(payload_dir)
            elif params is None:
                source_path = task.engine_state.get("torrent_path", "")
                if source_path and Path(source_path).is_file():
                    shutil.copy2(source_path, torrent_path)
                else:
                    await self._download_torrent_file(torrent_path)
                info = lt.torrent_info(str(torrent_path))
                params = lt.add_torrent_params()
                params.ti = info
                params.save_path = str(payload_dir)
            handle = session.add_torrent(params)
            self._handle = handle
            try:
                handle.set_max_connections(max(50, int(settings.bt_max_connections)))
            except Exception:
                pass
            for peer in task.engine_state.get("peers", []):
                try:
                    host, port = str(peer).rsplit(":", 1)
                    handle.connect_peer((host, int(port)))
                except (ValueError, RuntimeError):
                    continue

            metadata_deadline = asyncio.get_running_loop().time() + 120
            while not handle.status().has_metadata:
                if self._is_canceled():
                    raise asyncio.CancelledError
                if self._is_pausing():
                    handle.pause()
                    session.remove_torrent(handle)
                    self._handle = None
                    task.status = TaskStatus.PAUSED
                    self._set_stage("paused", "已暂停，BT 元数据将在恢复后继续获取")
                    return
                if asyncio.get_running_loop().time() >= metadata_deadline:
                    raise RuntimeError("获取 BT 元数据超时，请检查磁力链接、Tracker 或网络")
                await asyncio.sleep(0.5)

            info = handle.torrent_file()
            task.title = task.title or info.name()
            task.filename = task.filename or sanitize_filename(info.name())
            files = []
            storage = info.files()
            for index in range(storage.num_files()):
                entry = {
                    "index": index,
                    "path": storage.file_path(index).replace("\\", "/"),
                    "size": int(storage.file_size(index)),
                    "offset": int(storage.file_offset(index)),
                }
                files.append(entry)
            task.engine_state["files"] = files
            if "selected_files" not in task.engine_state:
                task.engine_state["selected_files"] = [entry["index"] for entry in files]
            self._apply_file_priorities()
            self._piece_length = int(info.piece_length())
            media_extensions = {
                ".mp4", ".m4v", ".webm", ".mkv", ".mov", ".ts",
                ".mp3", ".m4a", ".aac", ".flac", ".ogg",
            }
            selected_files = [
                entry for entry in files
                if entry["index"] in task.engine_state["selected_files"]
            ]
            stream_entry = next(
                (entry for entry in selected_files if Path(entry["path"]).suffix.lower() in media_extensions),
                selected_files[0] if selected_files else None,
            )
            if stream_entry is not None:
                self._stream_file_offset = int(stream_entry["offset"])
                self._stream_path = payload_dir / Path(stream_entry["path"])
                task.engine_state["stream_path"] = str(self._stream_path)
                task.engine_state["stream_size"] = int(stream_entry["size"])
                task.engine_state["stream_file_index"] = int(stream_entry["index"])
                task.engine_state["piece_length"] = self._piece_length
                task.engine_state["stream_file_offset"] = self._stream_file_offset
            task.progress.total_bytes = sum(
                entry["size"] for entry in files if entry["index"] in task.engine_state["selected_files"]
            )
            task.progress.total_segments = int(info.num_pieces())
            task.status = TaskStatus.DOWNLOADING
            task.progress.connection_status = "running"
            self._set_stage("downloading", f"BT 下载已开始，共 {len(files)} 个文件")

            while True:
                if self._is_canceled():
                    raise asyncio.CancelledError
                if self._is_pausing():
                    handle.pause()
                    await self._save_resume(lt, session, handle, resume_path)
                    session.remove_torrent(handle)
                    self._handle = None
                    task.status = TaskStatus.PAUSED
                    task.progress.connection_status = "idle"
                    self._set_stage("paused", "已暂停，BT 断点已保存")
                    return
                status = handle.status()
                task.progress.downloaded_bytes = int(status.total_wanted_done)
                task.progress.uploaded_bytes = int(status.total_upload)
                task.progress.speed_bytes_per_sec = float(status.download_rate)
                task.progress.upload_speed_bytes_per_sec = float(status.upload_rate)
                task.progress.peer_count = int(status.num_peers)
                task.progress.seed_count = int(status.num_seeds)
                task.progress.completed_segments = int(status.num_pieces)
                task.progress.progress_percent = max(0.0, min(100.0, float(status.progress) * 100))
                task.progress.eta_seconds = (
                    max(0.0, (task.progress.total_bytes - task.progress.downloaded_bytes) / task.progress.speed_bytes_per_sec)
                    if task.progress.speed_bytes_per_sec > 0
                    else 0.0
                )
                task.last_log = (
                    f"BT {task.progress.progress_percent:.1f}% · "
                    f"Peer {task.progress.peer_count} · Seed {task.progress.seed_count}"
                )
                self._publish()
                if self._priority_piece is not None:
                    self._prioritize_piece(self._priority_piece)
                if status.is_seeding or status.is_finished:
                    break
                error = getattr(status, "errc", None)
                if error and error.value() != 0:
                    raise RuntimeError(error.message())
                await asyncio.sleep(0.75)

            await self._flush_storage(lt, session, handle)
            handle.pause()
            await self._save_resume(lt, session, handle, resume_path)
            session.remove_torrent(handle)
            self._handle = None
            destination = self._move_payload(info, payload_dir)
            task.output_path = str(destination)
            task.engine_state["output_is_file"] = destination.is_file()
            if destination.is_file():
                task.engine_state["stream_path"] = str(destination)
            elif stream_entry is not None:
                relative = Path(stream_entry["path"])
                if relative.parts and relative.parts[0] == info.name():
                    relative = Path(*relative.parts[1:])
                task.engine_state["stream_path"] = str(destination / relative)
            task.progress.progress_percent = 100.0
            task.progress.connection_status = "idle"
            task.status = TaskStatus.DONE
            task.finished_at = datetime.now().isoformat()
            self._set_stage("done", f"BT 下载完成: {destination.name}")
            if not settings.keep_temp_files:
                await asyncio.to_thread(shutil.rmtree, task_dir, True)
        except asyncio.CancelledError:
            if self._handle is not None and self._session is not None:
                try:
                    self._session.remove_torrent(self._handle, 1)
                except Exception:
                    pass
            task.progress.connection_status = "idle"
            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                task.finished_at = datetime.now().isoformat()
                if not settings.keep_temp_files:
                    await asyncio.to_thread(shutil.rmtree, task_dir, True)
            else:
                task.status = TaskStatus.PAUSED
                task.stage = "interrupted"
                task.last_log = "程序已关闭，BT 数据已保留，可恢复"
                self._publish()
            raise
        except Exception as exc:
            details = diagnose_download_error(exc, stage=task.stage, url=task.url)
            task.error_code = "BT_DOWNLOAD_FAILED" if details.code == "DOWNLOAD_FAILED" else details.code
            task.error_stage = details.stage
            task.error_url = details.url
            task.error_hint = details.hint or "检查磁力链接、Tracker、DHT、磁盘空间和防火墙。"
            task.error_message = format_download_error(
                details.__class__(
                    code=task.error_code,
                    message=details.message,
                    hint=task.error_hint,
                    stage=details.stage,
                    url=details.url,
                )
            )
            task.status = TaskStatus.FAILED
            task.finished_at = datetime.now().isoformat()
            task.progress.connection_status = "error"
            self._set_stage("failed", task.error_message)
        finally:
            if self._handle is not None and self._session is not None:
                try:
                    self._session.remove_torrent(self._handle)
                except Exception:
                    pass
            self._handle = None
            self._session = None

    async def _download_torrent_file(self, destination: Path) -> None:
        headers = {
            "User-Agent": self.task.user_agent or settings.default_user_agent,
            "Referer": self.task.referer or settings.default_referer,
            "Cookie": self.task.cookie or settings.default_cookie,
        }
        async with httpx.AsyncClient(follow_redirects=True, timeout=30) as client:
            response = await client.get(self.task.url, headers={key: value for key, value in headers.items() if value})
            response.raise_for_status()
            if len(response.content) > 16 * 1024 * 1024:
                raise RuntimeError(".torrent 文件超过 16 MiB 限制")
            destination.write_bytes(response.content)

    def _apply_file_priorities(self) -> None:
        if (
            self._handle is None
            or not self._handle.is_valid()
            or not self._handle.status().has_metadata
        ):
            return
        selected = set(self.task.engine_state.get("selected_files", []))
        files = self.task.engine_state.get("files", [])
        priorities = [4 if entry["index"] in selected else 0 for entry in files]
        self._handle.prioritize_files(priorities)

    async def _save_resume(self, lt, session, handle, destination: Path) -> None:
        async with _RESUME_ALERT_LOCK:
            try:
                handle.save_resume_data()
                deadline = asyncio.get_running_loop().time() + 10
                while asyncio.get_running_loop().time() < deadline:
                    for alert in session.pop_alerts():
                        alert_handle = getattr(alert, "handle", None)
                        if alert_handle is not None and alert_handle != handle:
                            continue
                        if isinstance(alert, lt.save_resume_data_alert):
                            destination.write_bytes(bytes(lt.write_resume_data_buf(alert.params)))
                            return
                        if isinstance(alert, lt.save_resume_data_failed_alert):
                            return
                    await asyncio.sleep(0.1)
            except Exception:
                return

    async def _flush_storage(self, lt, session, handle, timeout: float = 30.0) -> None:
        """Wait until completed pieces are physically committed before moving files."""
        async with _RESUME_ALERT_LOCK:
            handle.flush_cache()
            deadline = asyncio.get_running_loop().time() + timeout
            while asyncio.get_running_loop().time() < deadline:
                for alert in session.pop_alerts():
                    alert_handle = getattr(alert, "handle", None)
                    if alert_handle is not None and alert_handle != handle:
                        continue
                    if isinstance(alert, lt.cache_flushed_alert):
                        return
                await asyncio.sleep(0.1)
        raise RuntimeError("BT 数据写入磁盘超时，临时文件已保留，可重试任务")

    def _move_payload(self, info, payload_dir: Path) -> Path:
        root = payload_dir / info.name()
        destination = task_output_dir(self.task) / sanitize_filename(info.name())
        for index in range(10000):
            candidate = destination if index == 0 else destination.with_name(f"{destination.stem}_{index}{destination.suffix}")
            if not candidate.exists():
                destination = candidate
                break
        if root.exists():
            publish_path(root, destination)
            return destination
        files = [path for path in payload_dir.rglob("*") if path.is_file()]
        if len(files) == 1:
            publish_path(files[0], destination)
            return destination
        destination.mkdir(parents=True, exist_ok=False)
        for path in files:
            relative = path.relative_to(payload_dir)
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            publish_path(path, target)
        return destination
