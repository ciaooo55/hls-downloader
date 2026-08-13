from __future__ import annotations

from typing import Mapping

CONNECTION_PARTS_LIMIT = 64
CONNECTION_PART_STATES = ("done", "active", "queued")


def _as_int(value, default=0):
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def _merge_intervals(intervals):
    ordered = sorted((start, end) for start, end in intervals if end > start)
    if not ordered:
        return []
    merged = [list(ordered[0])]
    for start, end in ordered[1:]:
        if start <= merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], end)
        else:
            merged.append([start, end])
    return [(start, end) for start, end in merged]


def _subtract(base, holes):
    if not base:
        return []
    if not holes:
        return list(base)
    remaining = []
    for start, end in base:
        pieces = [(start, end)]
        for hole_start, hole_end in holes:
            next_pieces = []
            for piece_start, piece_end in pieces:
                if hole_end <= piece_start or hole_start >= piece_end:
                    next_pieces.append((piece_start, piece_end))
                    continue
                if piece_start < hole_start:
                    next_pieces.append((piece_start, hole_start))
                if hole_end < piece_end:
                    next_pieces.append((hole_end, piece_end))
            pieces = next_pieces
        remaining.extend(pieces)
    return _merge_intervals(remaining)


def _covers(intervals, position):
    return any(start <= position < end for start, end in intervals)


def _paint_file_map(total, done_intervals, active_intervals):
    done = _subtract(_merge_intervals(done_intervals), active_intervals)
    active = _merge_intervals(active_intervals)
    points = {0, total}
    for start, end in done + active:
        if 0 <= start <= total:
            points.add(start)
        if 0 <= end <= total:
            points.add(end)
    ordered = sorted(points)
    parts = []
    for start, end in zip(ordered, ordered[1:]):
        if end <= start:
            continue
        if _covers(active, start):
            state, done_bytes = "active", end - start
        elif _covers(done, start):
            state, done_bytes = "done", end - start
        else:
            state, done_bytes = "queued", 0
        parts.append({"start": start, "end": end - 1, "done": done_bytes, "state": state})
    return parts


def _merge_pair(left, right):
    start = int(left["start"])
    end = int(right["end"])
    done = max(0, int(left["done"]) + int(right["done"]))
    size = end - start + 1
    states = {left["state"], right["state"]}
    if len(states) == 1:
        state = left["state"]
    elif "active" in states:
        state = "active"
    elif done >= size:
        state = "done"
    elif done > 0:
        state = "active"
    else:
        state = "queued"
    return {"start": start, "end": end, "done": min(size, done), "state": state}


def _bound_parts(parts, limit):
    if limit <= 0:
        return []
    merged = []
    for item in parts:
        if merged and merged[-1]["state"] == item["state"] and merged[-1]["end"] + 1 >= item["start"]:
            merged[-1] = _merge_pair(merged[-1], item)
        else:
            merged.append(dict(item))
    while len(merged) > limit:
        best_index = 0
        best_score = None
        for index in range(len(merged) - 1):
            left, right = merged[index], merged[index + 1]
            size = int(right["end"]) - int(left["start"]) + 1
            active_hit = int(left["state"] == "active") + int(right["state"] == "active")
            score = (active_hit, size)
            if best_score is None or score < best_score:
                best_score = score
                best_index = index
        merged[best_index] = _merge_pair(merged[best_index], merged[best_index + 1])
        del merged[best_index + 1]
    return merged


def normalize_connection_parts(values, *, total=0, limit=CONNECTION_PARTS_LIMIT):
    """Sanitize a display-only HTTP range map. Empty input stays empty."""
    if not isinstance(values, list):
        return []
    total = max(0, _as_int(total))
    parts = []
    for item in values:
        if not isinstance(item, Mapping):
            continue
        start = max(0, _as_int(item.get("start")))
        end = max(start, _as_int(item.get("end"), start))
        if total and start >= total:
            continue
        if total:
            end = min(end, total - 1)
        size = end - start + 1
        if size <= 0:
            continue
        done = min(size, max(0, _as_int(item.get("done"))))
        state = str(item.get("state") or "").strip().lower()
        if state not in CONNECTION_PART_STATES:
            state = "done" if done >= size else "active" if done > 0 else "queued"
        parts.append({"start": start, "end": end, "done": done, "state": state})
    parts.sort(key=lambda item: (item["start"], item["end"], item["state"]))
    return _bound_parts(parts, limit)


def build_connection_parts(*, total, chunks=None, range_current=None, completed=None, partials=None, finished_intervals=None, limit=CONNECTION_PARTS_LIMIT):
    """Build a non-overlapping file map from live HTTP engine ranges."""
    total = max(0, _as_int(total))
    if total <= 0:
        return []
    done_intervals = []
    active_intervals = []
    completed_indexes = {_as_int(item, -1) for item in (completed or [])}
    currents = {}
    if isinstance(range_current, Mapping):
        for key, value in range_current.items():
            currents[_as_int(key, -1)] = _as_int(value)
    chunk_list = list(chunks or [])
    if not chunk_list:
        return []
    for index, chunk in enumerate(chunk_list):
        try:
            start = int(chunk[0])
            end = int(chunk[1])
        except (TypeError, ValueError, IndexError):
            continue
        if end < start:
            continue
        current = currents.get(index, start)
        if index in completed_indexes or current > end:
            done_intervals.append((start, end + 1))
            continue
        if current > start:
            done_intervals.append((start, min(current, end + 1)))
        finished = finished_intervals.get(index) if isinstance(finished_intervals, Mapping) else None
        if isinstance(finished, Mapping):
            for finished_start, finished_end in finished.items():
                start_i = _as_int(finished_start)
                end_i = _as_int(finished_end)
                if end_i > start_i:
                    done_intervals.append((start_i, end_i))
    if isinstance(partials, Mapping):
        for key, received in partials.items():
            try:
                if not isinstance(key, (tuple, list)) or len(key) < 2:
                    continue
                part_start = int(key[1])
                part_done = int(received)
            except (TypeError, ValueError):
                continue
            if part_done > 0:
                active_intervals.append((part_start, part_start + part_done))
    return normalize_connection_parts(_paint_file_map(total, done_intervals, active_intervals), total=total, limit=limit)


def set_connection_parts(task, parts, *, total=None):
    progress = getattr(task, "progress", None)
    size = getattr(progress, "total_bytes", 0) if total is None and progress is not None else total
    payload = normalize_connection_parts(parts, total=_as_int(size), limit=CONNECTION_PARTS_LIMIT)
    if progress is not None:
        progress.connection_parts = payload
    return payload


def connection_parts_payload(task):
    progress = getattr(task, "progress", None)
    values = getattr(progress, "connection_parts", None) if progress is not None else None
    total = getattr(progress, "total_bytes", 0) if progress is not None else 0
    return normalize_connection_parts(values, total=_as_int(total), limit=CONNECTION_PARTS_LIMIT)


def count_active_parts(parts):
    return sum(1 for item in normalize_connection_parts(parts) if item["state"] == "active")
