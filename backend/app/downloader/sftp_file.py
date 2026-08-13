"""Ordinary SFTP file downloads.

One SSH session plus one SFTP file stream. Resume uses SFTP seek after STAT
succeeds and the remote identity still matches. This engine never
range-stitches, never consumes HTTP mirrors, and leaves FTP/HTTP paths
untouched.
"""
from __future__ import annotations

import asyncio
import getpass
import json
import shutil
import threading
import time
from collections import deque
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Protocol
from urllib.parse import unquote, urlsplit

from ..checksum import verify_task_checksum
from ..config import settings
from ..models import Task, TaskStatus
from ..naming import is_generic_media_name
from ..network_proxy import ensure_public_destination, ensure_url_allowed
from ..output_path import reserve_output_path
from ..paths import RUNTIME_PATHS
from ..utils import atomic_write_text, sanitize_filename
from .disk_space import MIN_FREE_RESERVE, ensure_download_capacity, ensure_free_space
from .engine import SeeklessEngine, publish_path, task_output_dir, task_work_dir
from .errors import diagnose_download_error
from .throttle import throttle_bytes

MAX_RETRIES = 5
CONNECT_TIMEOUT = 20.0
BLOCK_SIZE = 64 * 1024
STATE_VERSION = 1


class SftpError(RuntimeError):
    """User-visible SFTP failure with a stable Chinese message."""


class _SftpPaused(RuntimeError):
    pass


class _SftpCanceled(RuntimeError):
    pass


class SftpFile(Protocol):
    def read(self, size: int = -1) -> bytes: ...
    def seek(self, offset: int, whence: int = 0) -> int: ...
    def close(self) -> None: ...


class SftpClient(Protocol):
    def stat(self, path: str) -> Any: ...
    def open(self, filename: str, mode: str = "r") -> SftpFile: ...
    def close(self) -> None: ...


class SftpSession(Protocol):
    client: SftpClient
    def close(self) -> None: ...


@dataclass(frozen=True)
class SftpTarget:
    host: str
    port: int
    username: str
    password: str
    remote_path: str

    @property
    def resource_key(self) -> str:
        return f"sftp://{self.host}:{self.port}{self.remote_path}"

    @property
    def display_url(self) -> str:
        return self.resource_key


class _SpeedWindow:
    def __init__(self, span_seconds: float = 8.0) -> None:
        self._span = span_seconds
        self._samples: deque[tuple[float, int]] = deque()
        self._window_bytes = 0

    def _trim(self, now: float) -> None:
        cutoff = now - self._span
        while self._samples and self._samples[0][0] < cutoff:
            _, size = self._samples.popleft()
            self._window_bytes -= size

    def add(self, size: int) -> None:
        now = time.monotonic()
        self._samples.append((now, size))
        self._window_bytes += size
        self._trim(now)

    def speed(self) -> float:
        now = time.monotonic()
        self._trim(now)
        if not self._samples:
            return 0.0
        elapsed = max(now - self._samples[0][0], 0.25)
        return self._window_bytes / elapsed


def redact_sftp_url(value: str) -> str:
    raw = str(value or "").strip()
    if not raw:
        return ""
    try:
        parsed = urlsplit(raw)
        host = (parsed.hostname or "").rstrip(".")
        if not parsed.scheme or not host:
            return raw[:200]
        port = parsed.port
        netloc = f"{host}:{port}" if port else host
        return f"sftp://{netloc}{parsed.path or ''}"
    except ValueError:
        return raw.split("@")[-1][:200]


def parse_sftp_target(url: str) -> SftpTarget:
    raw = str(url or "").strip()
    try:
        parsed = urlsplit(raw)
    except ValueError as exc:
        raise SftpError("SFTP 地址无效") from exc
    if (parsed.scheme or "").lower() != "sftp":
        raise SftpError("链接必须是 sftp:// 地址")
    try:
        host = (parsed.hostname or "").rstrip(".").lower()
        port = parsed.port
    except ValueError as exc:
        raise SftpError("SFTP 主机或端口无效") from exc
    if not host:
        raise SftpError("SFTP 地址缺少有效主机名")
    username = unquote(parsed.username or "") or getpass.getuser() or ""
    if not username:
        raise SftpError("SFTP 地址需要用户名")
    password = unquote(parsed.password or "")
    remote_path = unquote(parsed.path or "")
    if not remote_path or remote_path.endswith("/"):
        raise SftpError("SFTP 地址必须指向具体文件，不能是目录")
    if chr(92) in remote_path or chr(0) in remote_path or any(ord(ch) < 32 for ch in remote_path):
        raise SftpError("SFTP 远程路径无效")
    return SftpTarget(
        host=host,
        port=int(port or 22),
        username=username,
        password=password,
        remote_path=remote_path,
    )


def sftp_filename(target: SftpTarget) -> str:
    name = Path(target.remote_path.replace("\\", "/")).name
    return sanitize_filename(name or "download")


def known_hosts_path() -> Path:
    return Path(RUNTIME_PATHS.data_root) / "sftp-known-hosts"


class _AppTofuPolicy:
    def __init__(self, path: Path) -> None:
        self.path = path

    def missing_host_key(self, client, hostname, key) -> None:
        import paramiko

        store = paramiko.HostKeys()
        if self.path.is_file():
            store.load(str(self.path))
        store.add(hostname, key.get_name(), key)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        store.save(str(self.path))
        client.get_host_keys().add(hostname, key.get_name(), key)


@dataclass
class _LiveSession:
    client: Any
    ssh: Any

    def close(self) -> None:
        try:
            self.client.close()
        except Exception:
            pass
        try:
            self.ssh.close()
        except Exception:
            pass


def open_sftp_session(target: SftpTarget, timeout: float = CONNECT_TIMEOUT) -> _LiveSession:
    try:
        import paramiko
    except ImportError as exc:
        raise SftpError("当前安装缺少 SFTP 组件 paramiko") from exc
    ssh = paramiko.SSHClient()
    try:
        ssh.load_system_host_keys()
    except Exception:
        pass
    store = known_hosts_path()
    if store.is_file():
        try:
            ssh.load_host_keys(str(store))
        except Exception:
            pass
    ssh.set_missing_host_key_policy(_AppTofuPolicy(store))
    try:
        ssh.connect(
            hostname=target.host,
            port=target.port,
            username=target.username,
            password=target.password or None,
            look_for_keys=not bool(target.password),
            allow_agent=not bool(target.password),
            timeout=timeout,
            auth_timeout=timeout,
            banner_timeout=timeout,
        )
        client = ssh.open_sftp()
    except Exception:
        try:
            ssh.close()
        except Exception:
            pass
        raise
    return _LiveSession(client=client, ssh=ssh)


def close_sftp_session(session: SftpSession | None) -> None:
    if session is None:
        return
    try:
        session.close()
    except Exception:
        pass


def describe_sftp_error(exc: BaseException) -> str:
    if isinstance(exc, SftpError):
        return str(exc)
    text = str(exc or "").strip() or type(exc).__name__
    lowered = text.lower()
    if "authentication" in lowered or "auth fail" in lowered or "permission denied" in lowered:
        return "SFTP 登录失败，请检查用户名、密码或私钥"
    if "not a valid" in lowered and "key" in lowered:
        return "SFTP 主机密钥不匹配，可能不是原来的服务器"
    if "timed out" in lowered or "timeout" in lowered:
        return "SFTP 连接超时"
    if "no such file" in lowered or "not found" in lowered:
        return "SFTP 远程文件不存在"
    if "connection refused" in lowered:
        return "SFTP 服务器拒绝连接"
    return f"SFTP 下载失败：{text[:200]}"


class SFTPDownloader(SeeklessEngine):
    def __init__(
        self,
        task: Task,
        on_progress=None,
        on_log=None,
        *,
        open_session: Callable[[SftpTarget], SftpSession] | None = None,
    ) -> None:
        self.task = task
        self.on_progress = on_progress or (lambda task: None)
        self.on_log = on_log or (lambda task_id, message: None)
        self._open_session = open_session or open_sftp_session
        self._part_path: Path | None = None
        self._progress_lock = threading.Lock()

    def _publish(self) -> None:
        self.on_progress(self.task)

    def _set_stage(self, stage: str, message: str) -> None:
        self.task.stage = stage
        self.task.last_log = message
        self.on_log(self.task.id, f"[{stage}] {message}")
        self._publish()

    def _is_canceled(self) -> bool:
        return bool(self.task.cancel_event and self.task.cancel_event.is_set())

    def _is_pausing(self) -> bool:
        return bool(self.task.pause_event and self.task.pause_event.is_set())

    def _apply_speed(self, window: _SpeedWindow) -> None:
        progress = self.task.progress
        speed = window.speed()
        progress.speed_bytes_per_sec = speed
        total = progress.total_bytes
        if total:
            progress.progress_percent = min(100.0, progress.downloaded_bytes * 100 / total)
            remaining = max(0, total - progress.downloaded_bytes)
            progress.eta_seconds = remaining / speed if speed > 0 else 0.0
        else:
            progress.eta_seconds = 0.0

    def _load_state(self, path: Path) -> dict[str, Any]:
        if not path.is_file():
            return {}
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {}
        return payload if isinstance(payload, dict) else {}

    def _save_state(self, path: Path, payload: dict[str, Any]) -> None:
        atomic_write_text(path, json.dumps(payload, ensure_ascii=False))

    async def _ensure_destination_policy(self, url: str) -> None:
        ensure_url_allowed(url)
        if self.task.engine_state.get("browser_originated"):
            await ensure_public_destination(url)

    def _probe(self, client: SftpClient, target: SftpTarget) -> dict[str, Any]:
        try:
            info = client.stat(target.remote_path)
        except FileNotFoundError as exc:
            raise SftpError("SFTP 远程文件不存在") from exc
        except OSError as exc:
            raise SftpError(describe_sftp_error(exc)) from exc
        total = int(getattr(info, "st_size", 0) or 0)
        mtime = str(int(getattr(info, "st_mtime", 0) or 0))
        if total < 0:
            total = 0
        return {"total": total, "mtime": mtime, "seek": True}

    def _transfer(
        self,
        client: SftpClient,
        target: SftpTarget,
        part_path: Path,
        *,
        offset: int,
        total: int,
    ) -> None:
        part_path.parent.mkdir(parents=True, exist_ok=True)
        mode = "r+b" if offset > 0 and part_path.exists() else "wb"
        if offset > 0 and not part_path.exists():
            offset = 0
            mode = "wb"
        remote = client.open(target.remote_path, "rb")
        try:
            if offset > 0:
                remote.seek(offset)
            with part_path.open(mode) as stream:
                if offset > 0:
                    stream.seek(offset)
                written = offset
                while True:
                    if self._is_canceled():
                        raise _SftpCanceled
                    if self._is_pausing():
                        raise _SftpPaused
                    chunk = remote.read(BLOCK_SIZE)
                    if not chunk:
                        break
                    wrote = stream.write(chunk)
                    if wrote != len(chunk):
                        raise OSError(f"本地文件写入不完整，期望 {len(chunk)} 字节，实际 {wrote} 字节")
                    written += len(chunk)
                    with self._progress_lock:
                        self.task.progress.downloaded_bytes = written
                        if total > 0:
                            self.task.progress.progress_percent = min(100.0, written * 100 / total)
                stream.flush()
        finally:
            try:
                remote.close()
            except Exception:
                pass

    async def _transfer_watched(
        self,
        client: SftpClient,
        target: SftpTarget,
        part_path: Path,
        *,
        offset: int,
        total: int,
    ) -> None:
        stop = threading.Event()
        error: list[BaseException] = []
        window = _SpeedWindow()
        last_bytes = offset
        loop = asyncio.get_running_loop()

        def worker() -> None:
            try:
                self._transfer(client, target, part_path, offset=offset, total=total)
            except BaseException as exc:
                error.append(exc)
            finally:
                loop.call_soon_threadsafe(stop.set)

        thread = threading.Thread(target=worker, name=f"sftp-{self.task.id}", daemon=True)
        thread.start()
        try:
            while not stop.is_set():
                current = self.task.progress.downloaded_bytes
                if current > last_bytes:
                    delta = current - last_bytes
                    await throttle_bytes(delta, self.task)
                    window.add(delta)
                    last_bytes = current
                    self._apply_speed(window)
                    self._publish()
                await asyncio.sleep(0.15)
        finally:
            thread.join(timeout=8.0)
        if self.task.progress.downloaded_bytes > last_bytes:
            await throttle_bytes(self.task.progress.downloaded_bytes - last_bytes, self.task)
            window.add(self.task.progress.downloaded_bytes - last_bytes)
            self._apply_speed(window)
            self._publish()
        if error:
            raise error[0]

    async def run(self) -> None:
        task = self.task
        task_dir = task_work_dir(task)
        task_dir.mkdir(parents=True, exist_ok=True)
        part_path = task_dir / "payload.downloading"
        self._part_path = part_path
        state_path = task_dir / "sftp-resume.json"
        output: Path | None = None
        session: SftpSession | None = None
        try:
            task.started_at = task.started_at or datetime.now().isoformat()
            task.status = TaskStatus.DOWNLOADING
            task.progress.connection_status = "connecting"
            task.progress.total_segments = 1
            task.progress.max_workers = 1
            task.progress.active_workers = 1
            self._set_stage("probing", "正在读取 SFTP 文件信息")
            target = parse_sftp_target(task.url)
            await self._ensure_destination_policy(target.display_url)
            session = await asyncio.to_thread(self._open_session, target)
            client = session.client
            try:
                metadata = await asyncio.to_thread(self._probe, client, target)
                total = int(metadata.get("total") or 0)
                mtime = str(metadata.get("mtime") or "")
                task.progress.total_bytes = total
                name = task.filename.strip()
                remote_name = sftp_filename(target)
                task.filename = sanitize_filename(
                    remote_name if not name or is_generic_media_name(name) else name
                )
                output = reserve_output_path(task_output_dir(task) / task.filename)
                task.engine_state["reserved_output_path"] = str(output)
                task.engine_state["stream_path"] = str(part_path)
                task.engine_state["total_size"] = total
                current_size = part_path.stat().st_size if part_path.exists() else 0
                if total > 0:
                    await asyncio.to_thread(
                        ensure_download_capacity, part_path, output, total, current_size=current_size
                    )
                else:
                    await asyncio.to_thread(ensure_free_space, part_path, MIN_FREE_RESERVE, operation="下载临时盘")
                state = self._load_state(state_path)
                offset = 0
                if (
                    total > 0
                    and current_size > 0
                    and current_size < total
                    and str(state.get("resource_key") or "") == target.resource_key
                    and int(state.get("total") or 0) == total
                    and str(state.get("mtime") or "") == mtime
                ):
                    offset = current_size
                    self._set_stage("downloading", f"SFTP 支持续传，从 {offset} 字节继续")
                else:
                    if part_path.exists() and (
                        total <= 0
                        or current_size >= total
                        or str(state.get("resource_key") or "") != target.resource_key
                        or int(state.get("total") or 0) != total
                    ):
                        part_path.unlink(missing_ok=True)
                    offset = 0
                    self._set_stage("downloading", "正在单连接下载 SFTP 文件")
                task.progress.downloaded_bytes = offset
                task.progress.connection_status = "running"
                self._publish()
                last_error: Exception | None = None
                for attempt in range(1, MAX_RETRIES + 1):
                    if self._is_canceled():
                        raise asyncio.CancelledError
                    if self._is_pausing():
                        task.status = TaskStatus.PAUSED
                        self._set_stage("paused", "已暂停，可继续下载")
                        return
                    try:
                        if attempt > 1:
                            close_sftp_session(session)
                            session = await asyncio.to_thread(self._open_session, target)
                            client = session.client
                            current_size = part_path.stat().st_size if part_path.exists() else 0
                            offset = current_size if current_size < (total or current_size + 1) else 0
                            if offset == 0 and part_path.exists():
                                part_path.unlink(missing_ok=True)
                        await self._transfer_watched(client, target, part_path, offset=offset, total=total)
                        last_error = None
                        break
                    except _SftpPaused:
                        self._save_state(
                            state_path,
                            {
                                "version": STATE_VERSION,
                                "resource_key": target.resource_key,
                                "total": total,
                                "mtime": mtime,
                                "offset": part_path.stat().st_size if part_path.exists() else 0,
                            },
                        )
                        task.status = TaskStatus.PAUSED
                        self._set_stage("paused", "已暂停，可继续下载")
                        return
                    except _SftpCanceled as exc:
                        raise asyncio.CancelledError from exc
                    except Exception as exc:
                        last_error = exc
                        if attempt >= MAX_RETRIES or self._is_canceled() or self._is_pausing():
                            break
                        delay = min(8.0, float(2 ** (attempt - 1)))
                        self._set_stage("downloading", f"SFTP 连接中断，{delay:.0f} 秒后重试（{attempt}/{MAX_RETRIES}）")
                        await asyncio.sleep(delay)
                if last_error is not None:
                    raise last_error
            finally:
                await asyncio.to_thread(close_sftp_session, session)
                session = None

            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                task.finished_at = datetime.now().isoformat()
                self._set_stage("canceled", "已取消")
                return
            if self._is_pausing():
                task.status = TaskStatus.PAUSED
                self._set_stage("paused", "已暂停，可继续下载")
                return
            if output is None:
                raise RuntimeError("下载输出路径未初始化")
            if not part_path.exists() or part_path.stat().st_size <= 0:
                raise SftpError("SFTP 下载结果为空")
            if task.progress.total_bytes and part_path.stat().st_size != task.progress.total_bytes:
                raise SftpError(
                    f"文件长度不匹配，期望 {task.progress.total_bytes}，实际 {part_path.stat().st_size}"
                )
            self._set_stage("verifying", "下载完成，正在写入并校验最终文件")
            await asyncio.to_thread(publish_path, part_path, output)
            state_path.unlink(missing_ok=True)
            task.output_path = str(output)
            task.engine_state["output_is_file"] = True
            task.engine_state.pop("reserved_output_path", None)
            task.engine_state["stream_path"] = str(output)
            task.engine_state["total_size"] = output.stat().st_size
            task.progress.downloaded_bytes = output.stat().st_size
            task.progress.completed_segments = 1
            if not await verify_task_checksum(task, output, on_progress=self.on_progress, on_log=self.on_log):
                return
            task.status = TaskStatus.DONE
            task.finished_at = datetime.now().isoformat()
            task.progress.progress_percent = 100.0
            task.progress.connection_status = "idle"
            task.progress.active_workers = 0
            self._set_stage("done", f"完成: {output.name}")
            if not settings.keep_temp_files:
                await asyncio.to_thread(shutil.rmtree, task_dir, True)
        except asyncio.CancelledError:
            task.progress.connection_status = "idle"
            task.progress.active_workers = 0
            if self._is_canceled():
                task.status = TaskStatus.CANCELED
                task.finished_at = datetime.now().isoformat()
                if not settings.keep_temp_files:
                    await asyncio.to_thread(shutil.rmtree, task_dir, True)
            else:
                task.status = TaskStatus.PAUSED
                task.stage = "interrupted"
                task.last_log = "程序已关闭，临时文件已保留，可恢复"
                self._publish()
            if output and output.exists() and output.stat().st_size == 0:
                output.unlink(missing_ok=True)
            raise
        except Exception as exc:
            details = diagnose_download_error(exc, stage=task.stage, url=redact_sftp_url(task.url), task_context=task)
            task.error_code = details.code or "SFTP_FAILED"
            task.error_stage = details.stage
            task.error_url = redact_sftp_url(task.url)
            task.error_hint = details.hint or describe_sftp_error(exc)
            task.error_message = describe_sftp_error(exc)
            task.status = TaskStatus.FAILED
            task.finished_at = datetime.now().isoformat()
            task.progress.connection_status = "error"
            task.progress.active_workers = 0
            self._set_stage("failed", task.error_message)
            if output and output.exists() and output.stat().st_size == 0:
                output.unlink(missing_ok=True)
        finally:
            if session is not None:
                await asyncio.to_thread(close_sftp_session, session)
            if output and task.status is not TaskStatus.DONE and output.exists() and output.stat().st_size == 0:
                output.unlink(missing_ok=True)
