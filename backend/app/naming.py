import html
import re
from pathlib import PurePosixPath
from urllib.parse import parse_qs, unquote, urlparse

from .utils import sanitize_filename


_MANIFEST_EXTENSIONS = re.compile(r"\.(?:m3u8?|mpd)$", re.IGNORECASE)
_GENERIC_STEM = re.compile(
    r"^(?:(?:video|stream|master|index|playlist|manifest|chunklist|media|output|download|file|vod|live)"
    r"(?:[-_ ]*(?:\d{3,4}p?|low|medium|high|sd|hd|fhd|uhd|4k))?|"
    r"(?:hls[-_ ]*)?(?:\d{3,4}p[-_ ]*)?(?:hls[-_ ]*)?(?:video[-_ ]*stream|视频流)?|"
    r"(?:hls[-_ ]*)?\d{3,4}p)$",
    re.IGNORECASE,
)
_OPAQUE_STEM = re.compile(r"^(?:[a-f0-9]{16,}|[a-z0-9_-]{28,})$", re.IGNORECASE)
_QUERY_NAME_KEYS = ("filename", "file", "title", "name", "download")


def _clean(value: str, *, path_value: bool = False) -> str:
    value = html.unescape(unquote(str(value or ""))).strip()
    if not value:
        return ""
    if path_value:
        value = value.replace("\\", "/").rsplit("/", 1)[-1]
    value = value.split("?", 1)[0].split("#", 1)[0]
    value = _MANIFEST_EXTENSIONS.sub("", value)
    return sanitize_filename(value) if value else ""


def _url_candidates(url: str) -> list[str]:
    try:
        parsed = urlparse(url)
    except ValueError:
        return []
    values: list[str] = []
    query = parse_qs(parsed.query)
    for key in _QUERY_NAME_KEYS:
        values.extend(query.get(key, []))
    leaf = PurePosixPath(unquote(parsed.path)).name
    if leaf:
        values.append(leaf)
    return values


def is_generic_media_name(value: str) -> bool:
    name = _clean(value, path_value=True)
    if not name:
        return True
    stem = name.rsplit(".", 1)[0] if "." in name else name
    compact = re.sub(r"\s+", "", stem)
    return bool(
        _GENERIC_STEM.fullmatch(stem)
        or _OPAQUE_STEM.fullmatch(compact)
        or compact.isdecimal()
    )


def suggest_manifest_name(
    url: str,
    *,
    filename: str = "",
    title: str = "",
    source_page_url: str = "",
    manifest_title: str = "",
    response_filename: str = "",
    fallback: str = "download",
) -> str:
    """Choose a human-readable HLS/DASH output name without a manifest suffix."""
    preferred = [
        _clean(response_filename, path_value=True),
        _clean(manifest_title),
        _clean(filename, path_value=True),
        _clean(title),
    ]
    page_candidates = [_clean(value, path_value=True) for value in _url_candidates(source_page_url)]
    url_candidates = [_clean(value, path_value=True) for value in _url_candidates(url)]
    candidates = [value for value in preferred + page_candidates + url_candidates if value]
    for value in candidates:
        if not is_generic_media_name(value):
            return value
    return candidates[0] if candidates else sanitize_filename(fallback)
