"""Ordinary FTP/FTPS file downloads.

A single control connection plus one data stream. REST resume is used only
after SIZE succeeds and the server accepts the restart offset. This engine
never range-stitches, never consumes HTTP mirrors, and leaves the HTTP/HLS
path untouched.
"""

from __future__ import annotations

import asyncio
import ftplib
import json
import shutil
import socket
import ssl
import threading
import time
from collections import deque
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from ..output_path import reserve_output_path
from typing import Any, Callable, Protocol
from urllib.parse import unquote, urlsplit

from ..checksum import verify_task_checksum
from ..config import settings
from ..models import Task, TaskStatus
from ..naming import is_generic_media_name
from ..network_proxy import ensure_public_destination, ensure_url_allowed
from ..utils import atomic_write_text, sanitize_filename
from .disk_space import MIN_FREE_RESERVE, ensure_download_capacity, ensure_free_space
from .engine import SeeklessEngine, publish_path, task_output_dir, task_work_dir
from .errors import diagnose_download_error, format_download_error
from .throttle import throttle_bytes


MAX_RETRIES = 5
CONNECT_TIMEOUT = 20.0
COMMAND_TIMEOUT = 60.0
BLOCK_SIZE = 64 * 1024
STATE_VERSION = 1


class FtpError(RuntimeError):
    """User-visible FTP failure with a stable Chinese message."""


class _FtpPaused(RuntimeError):
    pass


class _FtpCanceled(RuntimeError):
    pass


class FtpClient(Protocol):
    def login(self, user: str = "", passwd: str = "") -> str: ...
    def set_pasv(self, val: bool) -> None: ...
    def voidcmd(self, cmd: str) -> str: ...
    def sendcmd(self, cmd: str) -> str: ...
    def size(self, filename: str) -> int | None: ...
    def retrbinary(
        self,
        cmd: str,
        callback: Callable[[bytes], object],
        blocksize: int = 8192,
        rest: int | None = None,
    ) -> str: ...
    def quit(self) -> str: ...
    def close(self) -> None: ...


@dataclass(frozen=True)
class FtpTarget:
    scheme: str
    host: str
    port: int
    username: str
    password: str
    remote_path: str
    implicit_tls: bool

    @property
    def resource_key(self) -> str:
        return f"{self.scheme}://{self.host}:{self.port}{self.remote_path}"

    @property
    def display_url(self) -> str:
        return self.resource_key


class _ImplicitFTP_TLS(ftplib.FTP_TLS):
    """FTPS implicit TLS, used when the URL targets port 990."""

    def connect(self, host="", port=0, timeout=-999, source_address=None):
        if timeout != -999:
            self.timeout = timeout
        self.host = host
        self.port = port
        self.sock = socket.create_connection((self.host, self.port), self.timeout, source_address)
        self.af = self.sock.family
        context = self.context if getattr(self, "context", None) else ssl.create_default_context()
        self.sock = context.wrap_socket(self.sock, server_hostname=self.host)
        self.file = self.sock.makefile("r", encoding=self.encoding)
        self.welcome = self.getresp()
        return self.welcome


def redact_ftp_url(value: str) -> str:
    """Strip userinfo and query so logs never carry a password."""
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
        return f"{parsed.scheme.lower()}://{netloc}{parsed.path or ''}"
    except ValueError:
        return raw.split("@")[-1][:200]


def parse_ftp_target(url: str) -> FtpTarget:
    raw = str(url or "").strip()
    try:
        parsed = urlsplit(raw)
    except ValueError as exc:
        raise FtpError("FTP 地址无效") from exc
    scheme = (parsed.scheme or "").lower()
    if scheme not in {"ftp", "ftps"}:
        raise FtpError("链接必须是 ftp:// 或 ftps:// 地址")
    try:
        host = (parsed.hostname or "").rstrip(".").lower()
        port = parsed.port
    except ValueError as exc:
        raise FtpError("FTP 主机或端口无效") from exc
    if not host:
        raise FtpError("FTP 地址缺少有效主机名")
    implicit_tls = scheme == "ftps" and port == 990
    if port is None:
        port = 21
    username = unquote(parsed.username or "") or "anonymous"
    password = unquote(parsed.password or "")
    if username == "anonymous" and not password:
        password = "anonymous@"
    remote_path = unquote(parsed.path or "")
    if not remote_path or remote_path.endswith("/"):
        raise FtpError("FTP 地址必须指向具体文件，不能是目录")
    if chr(92) in remote_path or chr(0) in remote_path or any(ord(ch) < 32 for ch in remote_path):
        raise FtpError("FTP 远程路径无效")
    return FtpTarget(
        scheme=scheme,
        host=host,
        port=int(port),
        username=username,
        password=password,
        remote_path=remote_path,
        implicit_tls=implicit_tls,
    )


def ftp_filename(target: FtpTarget) -> str:
    name = Path(target.remote_path.replace("\\", "/")).name
    return sanitize_filename(name or "download")


def _reserve_output_path(path: Path) -> Path:
    return reserve_output_path(path)


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


def open_ftp_client(target: FtpTarget, timeout: float = COMMAND_TIMEOUT) -> FtpClient:
    if target.scheme == "ftps":
        client: ftplib.FTP = _ImplicitFTP_TLS(timeout=timeout) if target.implicit_tls else ftplib.FTP_TLS(timeout=timeout)
    else:
        client = ftplib.FTP(timeout=timeout)
    try:
        client.connect(target.host, target.port, timeout=min(timeout, CONNECT_TIMEOUT))
        if isinstance(client, ftplib.FTP_TLS) and not target.implicit_tls:
            client.auth()
        try:
            client.sendcmd("OPTS UTF8 ON")
            client.encoding = "utf-8"
        except ftplib.all_errors:
            pass
        client.login(target.username, target.password)
        if isinstance(client, ftplib.FTP_TLS):
            client.prot_p()
        client.set_pasv(True)
        client.voidcmd("TYPE I")
        return client
    except Exception:
        close_ftp_client(client)
        raise


def close_ftp_client(client: FtpClient | None) -> None:
    if client is None:
        return
    try:
        client.quit()
    except Exception:
        try:
            client.close()
        except Exception:
            pass


def ftp_size(client: FtpClient, remote_path: str) -> int:
    try:
        value = client.size(remote_path)
        return max(0, int(value or 0))
    except (TypeError, ValueError, ftplib.error_perm, ftplib.error_temp, ftplib.error_proto):
        return 0


def ftp_mdtm(client: FtpClient, remote_path: str) -> str:
    try:
        response = client.sendcmd(f"MDTM {remote_path}")
    except ftplib.all_errors:
        return ""
    parts = str(response or "").split()
    if len(parts) >= 2 and parts[0] == "213" and parts[1].isdigit():
        return parts[1]
    return ""


def describe_ftp_error(exc: BaseException) -> str:
    if isinstance(exc, FtpError):
        return str(exc)
    if isinstance(exc, ftplib.error_perm):
        text = str(exc)
        if text.startswith("530"):
            return "FTP 登录失败，请检查用户名和密码"
        if text.startswith("550"):
            return "FTP 服务器找不到该文件"
        if text.startswith("553"):
            return "FTP 服务器拒绝写入或文件名无效"
        return f"FTP 服务器拒绝请求：{text[:180]}"
    if isinstance(exc, (TimeoutError, socket.timeout)):
        return "FTP 连接超时"
    if isinstance(exc, socket.gaierror):
        return "无法解析 FTP 主机名"
    if isinstance(exc, ConnectionError):
        return "无法连接到 FTP 服务器"
    if isinstance(exc, ssl.SSLError):
        return "FTP TLS 握手失败"
    if isinstance(exc, OSError):
        return f"FTP 网络错误：{exc}"
    return f"FTP 下载失败：{type(exc).__name__}"


class FTPDownloader(SeeklessEngine):
    def __init__(
        self,
        task: Task,
        on_progress=None,
        on_log=None,
        *,
        open_client: Callable[[FtpTarget], FtpClient] | None = None,
    ) -> None:
        self.task = task
        self.on_progress = on_progress or (lambda task: None)
        self.on_log = on_log or (lambda task_id, message: None)
        self._open_client = open_client or (lambda target: open_ftp_client(target))
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

    def _probe(self, client: FtpClient, target: FtpTarget) -> dict[str, Any]:
        total = ftp_size(client, target.remote_path)
        mdtm = ftp_mdtm(client, target.remote_path)
        rest_ok = False
        if total > 0:
            try:
                client.sendcmd("REST 0")
                rest_ok = True
            except ftplib.all_errors:
                rest_ok = False
        return {"total": total, "mdtm": mdtm, "rest": rest_ok}

    def _transfer(
        self,
        client: FtpClient,
        target: FtpTarget,
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
        with part_path.open(mode) as stream:
            if offset > 0:
                stream.seek(offset)
            written = offset

            def callback(chunk: bytes) -> None:
                nonlocal written
                if self._is_canceled():
                    raise _FtpCanceled
                if self._is_pausing():
                    raise _FtpPaused
                if not chunk:
                    return
                wrote = stream.write(chunk)
                if wrote != len(chunk):
                    raise OSError(f"本地文件写入不完整，期望 {len(chunk)} 字节，实际 {wrote} 字节")
                written += len(chunk)
                with self._progress_lock:
                    self.task.progress.downloaded_bytes = written
                    if total > 0:
                        self.task.progress.progress_percent = min(100.0, written * 100 / total)

            command = f"RETR {target.remote_path}"
            rest = offset if offset > 0 else None
            client.retrbinary(command, callback, blocksize=BLOCK_SIZE, rest=rest)
            stream.flush()

    async def run(self) -> None:
        task = self.task
        task_dir = task_work_dir(task)
        task_dir.mkdir(parents=True, exist_ok=True)
        part_path = task_dir / "payload.downloading"
        self._part_path = part_path
        state_path = task_dir / "ftp-resume.json"
        output: Path | None = None
        try:
            task.started_at = task.started_at or datetime.now().isoformat()
            task.status = TaskStatus.DOWNLOADING
            task.progress.connection_status = "connecting"
            task.progress.total_segments = 1
            task.progress.max_workers = 1
            task.progress.active_workers = 1
            self._set_stage("probing", "正在读取 FTP 文件信息")
            target = parse_ftp_target(task.url)
            await self._ensure_destination_policy(target.display_url)
            client = await asyncio.to_thread(self._open_client, target)
            try:
                metadata = await asyncio.to_thread(self._probe, client, target)
                total = int(metadata.get("total") or 0)
                rest_ok = bool(metadata.get("rest"))
                mdtm = str(metadata.get("mdtm") or "")
                task.progress.total_bytes = total
                name = task.filename.strip()
                remote_name = ftp_filename(target)
                task.filename = sanitize_filename(
                    remote_name if not name or is_generic_media_name(name) else name
                )
                output = _reserve_output_path(task_output_dir(task) / task.filename)
                task.engine_state["reserved_output_path"] = str(output)
                task.engine_state["stream_path"] = str(part_path)
                task.engine_state["total_size"] = total
                current_size = part_path.stat().st_size if part_path.exists() else 0
                if total > 0:
                    await asyncio.to_thread(
                        ensure_download_capacity,
                        part_path,
                        output,
                        total,
                        current_size=current_size,
                    )
                else:
                    await asyncio.to_thread(
                        ensure_free_space,
                        part_path,
                        MIN_FREE_RESERVE,
                        operation="下载临时盘",
                    )
                state = self._load_state(state_path)
                offset = 0
                if (
                    rest_ok
                    and total > 0
                    and current_size > 0
                    and current_size < total
                    and str(state.get("resource_key") or "") == target.resource_key
                    and int(state.get("total") or 0) == total
                    and str(state.get("mdtm") or "") == mdtm
                ):
                    offset = current_size
                    self._set_stage("downloading", f"服务器支持续传，从 {offset} 字节继续")
                else:
                    if part_path.exists() and (
                        not rest_ok
                        or total <= 0
                        or current_size >= total
                        or str(state.get("resource_key") or "") != target.resource_key
                        or int(state.get("total") or 0) != total
                    ):
                        part_path.unlink(missing_ok=True)
                        current_size = 0
                    offset = 0
                    self._set_stage(
                        "downloading",
                        "正在单连接下载 FTP 文件" if rest_ok else "服务器未确认 REST，正在单连接下载",
                    )
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
                            close_ftp_client(client)
                            client = await asyncio.to_thread(self._open_client, target)
                            current_size = part_path.stat().st_size if part_path.exists() else 0
                            offset = current_size if rest_ok and current_size < (total or current_size + 1) else 0
                            if offset == 0 and part_path.exists():
                                part_path.unlink(missing_ok=True)
                        await self._transfer_watched(client, target, part_path, offset=offset, total=total)
                        last_error = None
                        break
                    except _FtpPaused:
                        self._save_state(
                            state_path,
                            {
                                "version": STATE_VERSION,
                                "resource_key": target.resource_key,
                                "total": total,
                                "mdtm": mdtm,
                                "offset": part_path.stat().st_size if part_path.exists() else 0,
                            },
                        )
                        task.status = TaskStatus.PAUSED
                        self._set_stage("paused", "已暂停，可继续下载")
                        return
                    except _FtpCanceled as exc:
                        raise asyncio.CancelledError from exc
                    except Exception as exc:
                        last_error = exc
                        if attempt >= MAX_RETRIES or self._is_canceled() or self._is_pausing():
                            break
                        delay = min(8.0, float(2 ** (attempt - 1)))
                        self._set_stage("downloading", f"FTP 连接中断，{delay:.0f} 秒后重试（{attempt}/{MAX_RETRIES}）")
                        await asyncio.sleep(delay)
                if last_error is not None:
                    raise last_error
            finally:
                await asyncio.to_thread(close_ftp_client, client)

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
                raise FtpError("FTP 下载结果为空")
            if task.progress.total_bytes and part_path.stat().st_size != task.progress.total_bytes:
                raise FtpError(
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
            details = diagnose_download_error(exc, stage=task.stage, url=redact_ftp_url(task.url), task_context=task)
            task.error_code = details.code or "FTP_FAILED"
            task.error_stage = details.stage
            task.error_url = redact_ftp_url(task.url)
            task.error_hint = details.hint or describe_ftp_error(exc)
            task.error_message = describe_ftp_error(exc) if isinstance(exc, (FtpError, ftplib.all_errors, OSError)) else format_download_error(details)
            task.status = TaskStatus.FAILED
            task.finished_at = datetime.now().isoformat()
            task.progress.connection_status = "error"
            task.progress.active_workers = 0
            self._set_stage("failed", task.error_message)
            if output and output.exists() and output.stat().st_size == 0:
                output.unlink(missing_ok=True)
        finally:
            if output and task.status is not TaskStatus.DONE and output.exists() and output.stat().st_size == 0:
                output.unlink(missing_ok=True)

    async def _transfer_watched(
        self,
        client: FtpClient,
        target: FtpTarget,
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

        thread = threading.Thread(target=worker, name=f"ftp-{self.task.id}", daemon=True)
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
