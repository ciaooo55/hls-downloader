from __future__ import annotations

from typing import Mapping

ENDGAME_SPLIT_MIN_BYTES = 512 * 1024


def pick_endgame_split(
    *,
    live_parts: Mapping[tuple[int, int], int],
    partials: Mapping[tuple[int, int], int] | None = None,
    completed: object = None,
    min_bytes: int = ENDGAME_SPLIT_MIN_BYTES,
) -> tuple[int, int, int, int] | None:
    """Choose the largest in-flight Range tail that an idle worker can steal.

    Returns (chunk_index, parent_start, split_start, split_end) or None.
    Display and resume still use the existing per-chunk checkpoint; this only
    decides which live part to shrink.
    """
    done = set()
    for item in completed or []:
        try:
            done.add(int(item))
        except (TypeError, ValueError):
            continue
    received_map = partials or {}
    best: tuple[int, int, int, int] | None = None
    best_remaining = max(1, int(min_bytes)) - 1
    for key, stop in live_parts.items():
        try:
            index, start = int(key[0]), int(key[1])
            stop_at = int(stop)
        except (TypeError, ValueError, IndexError):
            continue
        if index in done or stop_at < start:
            continue
        try:
            received = max(0, int(received_map.get((index, start), 0) or 0))
        except (TypeError, ValueError):
            received = 0
        remaining = stop_at - (start + received) + 1
        if remaining <= best_remaining:
            continue
        split_start = start + received + remaining // 2
        if split_start <= start + received or split_start > stop_at:
            continue
        best = (index, start, split_start, stop_at)
        best_remaining = remaining
    return best
