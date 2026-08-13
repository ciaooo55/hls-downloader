from __future__ import annotations

RESUME_STATUSES = {"paused", "pausing"}
RETRY_STATUSES = {"failed", "canceled", "unsupported"}
START_STATUSES = {"queued", "awaiting_selection", "awaiting_confirmation"}
ACTIVE_STATUSES = {
    "downloading",
    "downloading_segments",
    "downloading_m3u8",
    "fetching_metadata",
    "checking",
    "parsing",
    "merging",
    "remuxing",
}


def suggest_duplicate_action(
    status: object,
    actions: object = None,
    *,
    output_missing: bool = False,
    output_path: str = "",
) -> str:
    """Pick the IDM-style reuse action for an existing same-URL task."""
    state = str(getattr(status, "value", status) or "")
    available = {str(item) for item in (actions or [])}
    if state in RESUME_STATUSES and "resume" in available:
        return "resume"
    if state in RETRY_STATUSES and "retry" in available:
        return "retry"
    if state in START_STATUSES and "start" in available:
        return "start"
    if state == "done":
        if output_missing and "retry" in available:
            return "retry"
        if "open" in available or str(output_path or "").strip():
            return "open"
    if state in ACTIVE_STATUSES:
        return "focus"
    return "none"


def duplicate_task_entry(task, actions: object = None, *, output_missing: bool | None = None) -> dict:
    status = getattr(getattr(task, "status", ""), "value", getattr(task, "status", "")) or ""
    output_path = str(getattr(task, "output_path", "") or "")
    missing = bool(getattr(task, "output_missing", False) if output_missing is None else output_missing)
    available = list(actions or [])
    return {
        "id": str(getattr(task, "id", "") or ""),
        "status": str(status),
        "filename": str(getattr(task, "filename", "") or getattr(task, "title", "") or ""),
        "output_path": output_path,
        "updated_at": str(getattr(task, "updated_at", "") or getattr(task, "created_at", "") or ""),
        "available_actions": available,
        "output_missing": missing,
        "suggested_action": suggest_duplicate_action(
            status,
            available,
            output_missing=missing,
            output_path=output_path,
        ),
    }
