"""IDM-style download categories shared by desktop and browser tasks."""

from __future__ import annotations

from pathlib import Path

from .config import settings

CATEGORIES = ("media", "program", "archive", "other")
CATEGORY_LABELS = {
    "media": "媒体",
    "program": "程序",
    "archive": "压缩包",
    "other": "其他",
}
_ARCHIVE = {"zip", "7z", "rar", "tar", "gz", "bz2", "xz", "iso"}
_EXECUTABLE = {"exe", "msi", "msix", "appx", "bat", "cmd"}
_VIDEO = {"mp4", "mkv", "webm", "mov", "avi", "m4v", "ts"}
_AUDIO = {"mp3", "m4a", "aac", "flac", "wav", "ogg"}
_IMAGE = {"jpg", "jpeg", "png", "gif", "webp", "bmp", "svg"}


def _extension(path: str) -> str:
    name = str(path or "").split("?", 1)[0].split("#", 1)[0]
    name = name.replace("\\", "/").rsplit("/", 1)[-1]
    if "." not in name:
        return ""
    return name.rsplit(".", 1)[-1].lower()[:5]


def download_category(path: str = "", mime_type: str = "", task_type: str = "") -> str:
    if str(task_type or "") in {"hls", "dash"}:
        return "media"
    extension = _extension(path)
    if extension in _VIDEO or extension in _AUDIO or extension in _IMAGE:
        return "media"
    if extension in _EXECUTABLE:
        return "program"
    if extension in _ARCHIVE:
        return "archive"
    mime = str(mime_type or "").lower()
    if mime.startswith(("video/", "audio/", "image/")):
        return "media"
    return "other"


def resolve_category_output_dir(
    *,
    filename: str = "",
    url: str = "",
    mime_type: str = "",
    task_type: str = "",
    category: str = "",
    explicit_dir: str = "",
) -> str:
    """Return a category folder when policy applies; otherwise empty.

    An explicit directory wins unless it is exactly the global download root.
    That lets browser/desktop forms prefill the default folder without disabling
    automatic classification. A user-selected different folder is never moved.
    """
    chosen = str(category or download_category(filename or url, mime_type, task_type))
    if chosen not in CATEGORY_LABELS:
        chosen = "other"
    explicit = str(explicit_dir or "").strip()
    default_root = Path(settings.download_dir).expanduser().resolve()
    if explicit:
        explicit_path = Path(explicit).expanduser().resolve()
        if explicit_path != default_root:
            return str(explicit_path)
    configured = str((getattr(settings, "browser_category_dirs", {}) or {}).get(chosen) or "").strip()
    if configured:
        return str(Path(configured).expanduser().resolve())
    if getattr(settings, "auto_category_dirs", False):
        return str(default_root / CATEGORY_LABELS[chosen])
    return ""
