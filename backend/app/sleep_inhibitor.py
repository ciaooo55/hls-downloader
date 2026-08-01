from __future__ import annotations

import ctypes
import os
import threading


ES_CONTINUOUS = 0x80000000
ES_SYSTEM_REQUIRED = 0x00000001
ES_AWAYMODE_REQUIRED = 0x00000040


class SleepInhibitor:
    """Keep Windows awake only while downloads/recordings are active."""

    def __init__(self) -> None:
        self._active = False
        self._thread_id: int | None = None

    @property
    def active(self) -> bool:
        return self._active

    def update(self, active: bool) -> None:
        active = bool(active)
        if active == self._active:
            return
        if os.name == "nt":
            flags = ES_CONTINUOUS
            if active:
                flags |= ES_SYSTEM_REQUIRED | ES_AWAYMODE_REQUIRED
            result = ctypes.windll.kernel32.SetThreadExecutionState(flags)
            if not result:
                # Some systems reject away mode (notably battery/Modern
                # Standby policies); keeping the system awake still works.
                flags = ES_CONTINUOUS | (ES_SYSTEM_REQUIRED if active else 0)
                ctypes.windll.kernel32.SetThreadExecutionState(flags)
            self._thread_id = threading.get_ident()
        self._active = active

    def close(self) -> None:
        self.update(False)


sleep_inhibitor = SleepInhibitor()
