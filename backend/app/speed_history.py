from __future__ import annotations

import time
from typing import Iterable

SPEED_HISTORY_LIMIT = 180
SPEED_SAMPLE_INTERVAL = 1.0
TRANSFER_STATUSES = {
    "downloading",
    "downloading_segments",
    "downloading_m3u8",
    "fetching_metadata",
    "checking",
    "parsing",
}


def _status_value(task) -> str:
    status = getattr(task, "status", "")
    return str(getattr(status, "value", status) or "")


def _current_speed(task) -> int:
    progress = getattr(task, "progress", None)
    raw = getattr(progress, "speed_bytes_per_sec", 0) if progress is not None else 0
    try:
        return max(0, int(round(float(raw or 0))))
    except (TypeError, ValueError):
        return 0


def record_speed_sample(
    task,
    now: float | None = None,
    *,
    min_interval: float = SPEED_SAMPLE_INTERVAL,
    limit: int = SPEED_HISTORY_LIMIT,
) -> bool:
    """Record at most one sample per second. Display-only; does not throttle."""
    history = getattr(task, "speed_history", None)
    if history is None:
        history = []
        task.speed_history = history
    current = time.monotonic() if now is None else now
    last_at = float(getattr(task, "speed_history_at", 0) or 0)
    transferring = _status_value(task) in TRANSFER_STATUSES
    speed = _current_speed(task) if transferring else 0
    if last_at and current - last_at < min_interval:
        return False
    if not transferring:
        if not history or history[-1] == 0:
            return False
    history.append(speed)
    if len(history) > limit:
        del history[:-limit]
    task.speed_history = history
    task.speed_history_at = current
    peak = max(int(getattr(task, "speed_peak_bytes_per_sec", 0) or 0), speed)
    task.speed_peak_bytes_per_sec = peak
    return True


def record_speed_samples(tasks: Iterable[object], now: float | None = None) -> int:
    recorded = 0
    stamp = time.monotonic() if now is None else now
    for task in tasks:
        if record_speed_sample(task, stamp):
            recorded += 1
    return recorded


def speed_history_payload(task) -> list[int]:
    history = getattr(task, "speed_history", None) or []
    return [max(0, int(value or 0)) for value in history[-SPEED_HISTORY_LIMIT:]]


def speed_peak_payload(task) -> int:
    try:
        return max(0, int(getattr(task, "speed_peak_bytes_per_sec", 0) or 0))
    except (TypeError, ValueError):
        return 0
