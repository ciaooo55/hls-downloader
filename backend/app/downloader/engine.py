from __future__ import annotations

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
