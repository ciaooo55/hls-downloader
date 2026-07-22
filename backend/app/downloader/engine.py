from __future__ import annotations

import errno
import os
import shutil
import uuid
from pathlib import Path
from typing import Protocol

from ..config import settings


class DownloadEngine(Protocol):
    async def run(self) -> None: ...

    def request_seek(self, value: int) -> None: ...


class SeeklessEngine:
    def request_seek(self, value: int) -> None:
        return None


def task_output_dir(task) -> Path:
    raw = str(task.engine_state.get("output_dir", "") or settings.download_dir)
    output_dir = Path(raw).expanduser().resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    return output_dir


def task_temp_root(task=None) -> Path:
    """Return the process-file root fixed when a task is created.

    Tasks created before the separate temp directory setting was introduced keep
    using the old download directory so their resumable data remains available.
    """
    raw = ""
    if task is not None and not isinstance(task, str):
        raw = str(task.engine_state.get("temp_dir", "") or "")
    if not raw:
        raw = str(settings.download_dir if task is not None else settings.temp_dir)
    root = (Path(raw).expanduser().resolve() / ".tasks").resolve()
    root.mkdir(parents=True, exist_ok=True)
    return root


def task_work_dir(task_or_id) -> Path:
    task_id = str(task_or_id if isinstance(task_or_id, str) else task_or_id.id)
    root = task_temp_root(None if isinstance(task_or_id, str) else task_or_id)
    task_dir = (root / task_id).resolve()
    if task_dir.parent != root:
        raise ValueError("无效的任务目录")
    if isinstance(task_or_id, str) and not task_dir.exists():
        legacy_root = (Path(settings.download_dir).expanduser().resolve() / ".tasks").resolve()
        legacy_dir = (legacy_root / task_id).resolve()
        if legacy_dir.parent == legacy_root and legacy_dir.exists():
            return legacy_dir
    return task_dir


def temp_roots() -> tuple[Path, ...]:
    preferred = (Path(settings.temp_dir).expanduser().resolve() / ".tasks").resolve()
    legacy = (Path(settings.download_dir).expanduser().resolve() / ".tasks").resolve()
    return (preferred,) if preferred == legacy else (preferred, legacy)


def publish_path(source: Path, destination: Path) -> None:
    """Publish a completed file or directory, including across Windows drives."""
    source = Path(source)
    destination = Path(destination)
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.replace(source, destination)
        return
    except OSError as exc:
        if exc.errno != errno.EXDEV and getattr(exc, "winerror", None) != 17:
            raise

    staging = destination.with_name(f".{destination.name}.{uuid.uuid4().hex}.copying")
    try:
        if source.is_dir():
            shutil.copytree(source, staging)
        else:
            shutil.copy2(source, staging)
        os.replace(staging, destination)
        if source.is_dir():
            shutil.rmtree(source)
        else:
            source.unlink()
    finally:
        if staging.is_dir():
            shutil.rmtree(staging, ignore_errors=True)
        else:
            staging.unlink(missing_ok=True)
