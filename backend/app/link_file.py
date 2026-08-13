from __future__ import annotations

import re
from pathlib import Path
from urllib.parse import urlparse

MAX_LINK_FILE_BYTES = 256 * 1024
MAX_LINK_URLS = 100
WATCH_LINK_SUFFIXES = {".url", ".magnet"}
WATCH_FILE_SUFFIXES = {".torrent", ".url", ".magnet"}
EXPLORER_LINK_SUFFIXES = {".url", ".magnet", ".m3u", ".m3u8", ".mpd", ".html", ".htm", ".metalink", ".meta4"}
_SEGMENT_SUFFIXES = (".ts", ".m4s", ".cmfv", ".cmfa")
_PLAYLIST_SUFFIXES = (".m3u8", ".m3u", ".mpd")


class LinkFileError(ValueError):
    pass


def decode_link_bytes(data: bytes) -> str:
    if data.startswith(b"\xff\xfe") or data.startswith(b"\xfe\xff"):
        return data.decode("utf-16")
    if data.startswith(b"\xef\xbb\xbf"):
        return data[3:].decode("utf-8")
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError:
        return data.decode("utf-16", errors="replace")


def normalize_download_url(raw: str) -> str:
    url = str(raw or "").strip().strip("\ufeff").strip('"').strip("'")
    if not url or any(ord(character) < 32 for character in url if character not in "\t"):
        raise LinkFileError("link is empty")
    parsed = urlparse(url)
    scheme = (parsed.scheme or "").lower()
    if scheme == "magnet":
        if "xt=" not in (parsed.query or "").lower():
            raise LinkFileError("magnet link is incomplete")
        return url
    if scheme in {"http", "https", "ftp", "ftps", "sftp"} and parsed.hostname:
        return url
    raise LinkFileError("unsupported link scheme")


_ABS_URL_PATTERN = re.compile(
    r'(?:https?://|ftps?://|sftp://|magnet:\?)[^\s<>"]+',
    re.IGNORECASE,
)


def _path_suffix(url: str) -> str:
    path = urlparse(url).path.lower()
    if "." not in path.rsplit("/", 1)[-1]:
        return ""
    return "." + path.rsplit(".", 1)[-1]


def _is_segment_url(url: str) -> bool:
    return _path_suffix(url) in _SEGMENT_SUFFIXES


def collect_absolute_urls(text: str, limit: int = MAX_LINK_URLS) -> list[str]:
    found: list[str] = []
    seen: set[str] = set()
    for match in _ABS_URL_PATTERN.finditer(str(text or "")):
        raw = match.group(0).rstrip(".,);]")
        try:
            url = normalize_download_url(raw)
        except LinkFileError:
            continue
        key = url.lower()
        if key in seen:
            continue
        seen.add(key)
        found.append(url)
        if len(found) >= limit:
            break
    return found


def extract_text_urls(text: str, limit: int = MAX_LINK_URLS) -> list[str]:
    found: list[str] = []
    seen: set[str] = set()
    for line in str(text or "").splitlines():
        candidate = line.strip()
        if not candidate or candidate.startswith("#") or candidate.startswith(";"):
            continue
        try:
            url = normalize_download_url(candidate)
        except LinkFileError:
            continue
        key = url.lower()
        if key in seen:
            continue
        seen.add(key)
        found.append(url)
        if len(found) >= limit:
            break
    if not found:
        raise LinkFileError("text file has no download link")
    return found


def url_from_internet_shortcut(text: str) -> str:
    for line in str(text or "").splitlines():
        stripped = line.strip()
        if stripped.lower().startswith("url="):
            return normalize_download_url(stripped[4:])
    raise LinkFileError("shortcut has no URL=")


def url_from_plain_text(text: str) -> str:
    for line in str(text or "").splitlines():
        candidate = line.strip()
        if not candidate or candidate.startswith("#") or candidate.startswith(";"):
            continue
        return normalize_download_url(candidate)
    raise LinkFileError("text file has no download link")


def extract_playlist_urls(text: str) -> list[str]:
    urls = collect_absolute_urls(text)
    if not urls:
        raise LinkFileError("playlist has no remote download link")
    upper = str(text or "").upper()
    playlists = [url for url in urls if _path_suffix(url) in _PLAYLIST_SUFFIXES]
    if "#EXT-X-STREAM-INF" in upper:
        return playlists or urls
    if "#EXTINF" in upper:
        if playlists:
            return playlists
        segment_count = sum(1 for url in urls if _is_segment_url(url))
        if segment_count >= 3 or (urls and segment_count == len(urls)):
            raise LinkFileError("\u8fd9\u662f\u672c\u5730\u5206\u7247\u64ad\u653e\u5217\u8868\uff0c\u8bf7\u6539\u7528\u7f51\u9875\u6216\u8fdc\u7a0b m3u8 \u5730\u5740")
    files = [url for url in urls if not _is_segment_url(url)]
    return files or urls


def extract_mpd_urls(text: str) -> list[str]:
    urls = collect_absolute_urls(text)
    playlists = [url for url in urls if _path_suffix(url) == ".mpd"]
    if playlists:
        return playlists
    files = [url for url in urls if not _is_segment_url(url)]
    if files:
        return files
    raise LinkFileError("DASH \u6e05\u5355\u91cc\u6ca1\u6709\u53ef\u5355\u72ec\u4e0b\u8f7d\u7684\u8fdc\u7a0b\u5730\u5740")


def extract_html_urls(text: str) -> list[str]:
    from .page_harvest import extract_page_links

    links, _title, _truncated = extract_page_links(str(text or ""), "https://hls-downloader.invalid/")
    urls = [item.url for item in links if "hls-downloader.invalid" not in item.url.lower()]
    if urls:
        return urls[:MAX_LINK_URLS]
    raise LinkFileError("\u7f51\u9875\u6587\u4ef6\u91cc\u6ca1\u6709\u53ef\u4e0b\u8f7d\u7684\u8fdc\u7a0b\u94fe\u63a5")


def extract_download_urls(text: str, *, suffix: str = "") -> list[str]:
    ext = str(suffix or "").lower()
    body = str(text or "")
    if ext == ".url" or "[internetshortcut]" in body.lower():
        return [url_from_internet_shortcut(body)]
    if ext == ".magnet":
        return [url_from_plain_text(body)]
    if ext in {".m3u", ".m3u8"}:
        return extract_playlist_urls(body)
    if ext == ".mpd":
        return extract_mpd_urls(body)
    if ext in {".html", ".htm"}:
        return extract_html_urls(body)
    if ext == ".txt":
        return extract_text_urls(body)
    return [extract_download_url(body, suffix=suffix)]


def read_link_urls(path: Path) -> list[str]:
    source = Path(path)
    if not source.is_file():
        raise LinkFileError("not a file")
    size = source.stat().st_size
    if size <= 0 or size > MAX_LINK_FILE_BYTES:
        raise LinkFileError("link file is empty or too large")
    data = source.read_bytes()
    if not data or len(data) > MAX_LINK_FILE_BYTES:
        raise LinkFileError("link file is empty or too large")
    if source.suffix.lower() in {".metalink", ".meta4"}:
        from .metalink import read_metalink_files
        return [item.url for item in read_metalink_files(source)]
    return extract_download_urls(decode_link_bytes(data), suffix=source.suffix)


def extract_download_url(text: str, *, suffix: str = "") -> str:
    ext = str(suffix or "").lower()
    body = str(text or "")
    if ext in {".m3u", ".m3u8", ".mpd", ".html", ".htm", ".txt"}:
        return extract_download_urls(body, suffix=ext)[0]
    if ext == ".url" or "[internetshortcut]" in body.lower():
        return url_from_internet_shortcut(body)
    if ext == ".magnet":
        return url_from_plain_text(body)
    lowered = body.lstrip("\ufeff ").lower()
    if lowered.startswith("[internetshortcut]") or "\nurl=" in "\n" + lowered:
        return url_from_internet_shortcut(body)
    return url_from_plain_text(body)


def read_link_file(path: Path) -> str:
    return read_link_urls(path)[0]