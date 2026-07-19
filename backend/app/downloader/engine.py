from __future__ import annotations

from typing import Protocol


class DownloadEngine(Protocol):
    async def run(self) -> None: ...

    def request_seek(self, value: int) -> None: ...


class SeeklessEngine:
    def request_seek(self, value: int) -> None:
        return None
