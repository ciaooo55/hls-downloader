"""Single-page download-link harvest.

Opt-in site grabber: fetch one HTML page, extract static file/magnet/FTP
links, and return a confirmable list. No JavaScript execution, no recursive
crawl, no per-link HEAD probing, and no change to the single-URL recognize
or create path.
"""

from __future__ import annotations

import asyncio
import html
import re
from html.parser import HTMLParser
from typing import Literal
from urllib.parse import parse_qsl, unquote, urljoin, urlparse, urlunparse

import httpx
from pydantic import BaseModel

from .network_proxy import ensure_url_allowed, policy_httpx_client
from .url_recognition import MAX_RESPONSE_BYTES, _is_dash_manifest, _is_direct_file_response
from .utils import sanitize_filename


class HarvestError(RuntimeError):
    pass


MAX_HARVEST_LINKS = 100
MAX_RAW_LINKS = 512
_TITLE_LIMIT = 200

DEFAULT_EXTENSIONS = frozenset({
    "mp4", "mkv", "webm", "mov", "avi", "m4v", "ts", "flv",
    "mp3", "m4a", "aac", "flac", "wav", "ogg", "opus",
    "zip", "7z", "rar", "tar", "gz", "bz2", "xz", "iso",
    "exe", "msi", "msix", "appx", "dmg", "apk", "deb", "rpm",
    "pdf", "epub", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "m3u8", "mpd", "m3u", "torrent", "bin",
})
_VIDEO_EXTS = frozenset({"mp4", "mkv", "webm", "mov", "avi", "m4v", "ts", "flv"})
_AUDIO_EXTS = frozenset({"mp3", "m4a", "aac", "flac", "wav", "ogg", "opus"})
_ARCHIVE_EXTS = frozenset({"zip", "7z", "rar", "tar", "gz", "bz2", "xz", "iso"})
_PROGRAM_EXTS = frozenset({"exe", "msi", "msix", "appx", "dmg", "apk", "deb", "rpm"})
_DOCUMENT_EXTS = frozenset({"pdf", "epub", "doc", "docx", "xls", "xlsx", "ppt", "pptx"})
_PLAYLIST_EXTS = frozenset({"m3u8", "mpd", "m3u"})
_QUERY_NAME_KEYS = {"filename", "file", "download", "name", "title"}
_ABS_URL_PATTERN = re.compile(
    r'(?:https?://|ftps?://|sftp://|magnet:\?)[^\s<>"]+',
    re.IGNORECASE,
)
_PLACEHOLDER_PATTERN = re.compile(
    r'(?:\$\{|\{\{|\}\}|<%|%7b|%7d|\[object(?:%20|\s)+object\]|(?:^|[/_.-])(?:undefined|null)(?:[/_.?-]|$))',
    re.IGNORECASE,
)
_NOT_PAGE_MESSAGES = {
    "file": "\u8fd9\u662f\u76f4\u63a5\u6587\u4ef6\u5730\u5740\uff0c\u4e0d\u662f\u7f51\u9875\u3002\u8bf7\u7528\u65b0\u5efa\u4e0b\u8f7d\u3002",
    "hls": "\u8fd9\u662f HLS \u64ad\u653e\u6e05\u5355\uff0c\u4e0d\u662f\u7f51\u9875\u3002\u8bf7\u7528\u65b0\u5efa\u4e0b\u8f7d\u3002",
    "dash": "\u8fd9\u662f DASH \u64ad\u653e\u6e05\u5355\uff0c\u4e0d\u662f\u7f51\u9875\u3002\u8bf7\u7528\u65b0\u5efa\u4e0b\u8f7d\u3002",
}


class HarvestLink(BaseModel):
    url: str
    filename: str = ""
    label: str = ""
    extension: str = ""
    category: str = "other"
    source: str = "href"


class HarvestResult(BaseModel):
    kind: Literal["page", "file", "hls", "dash", "none"]
    page_url: str
    final_url: str
    title: str = ""
    links: list[HarvestLink]
    truncated: bool = False
    message: str = ""


def normalize_harvest_extensions(values: list[str] | None) -> set[str]:
    cleaned: set[str] = set()
    for item in values or []:
        ext = str(item or "").strip().lower().lstrip(".")
        if ext and len(ext) <= 8 and ext.isalnum():
            cleaned.add(ext)
    return cleaned or set(DEFAULT_EXTENSIONS)


def harvest_category(extension: str, url: str = "") -> str:
    if str(url or "").lower().startswith("magnet:"):
        return "torrent"
    ext = str(extension or "").lower()
    if ext in _VIDEO_EXTS:
        return "video"
    if ext in _AUDIO_EXTS:
        return "audio"
    if ext in _ARCHIVE_EXTS:
        return "archive"
    if ext in _PROGRAM_EXTS:
        return "program"
    if ext in _DOCUMENT_EXTS:
        return "document"
    if ext in _PLAYLIST_EXTS:
        return "playlist"
    if ext == "torrent":
        return "torrent"
    return "other"

def _decode_url_escapes(value: str) -> str:
    value = html.unescape(value).replace("\\/", "/")
    replacements = {
        "\\u0026": "&", "\\u002f": "/", "\\u003a": ":", "\\u003d": "=",
        "\\x26": "&", "\\x2f": "/", "\\x3a": ":", "\\x3d": "=",
    }
    for escaped, decoded in replacements.items():
        value = re.sub(re.escape(escaped), decoded, value, flags=re.IGNORECASE)
    return value


def _clean_raw_url(value: str) -> str:
    value = _decode_url_escapes(value).strip().strip('"\'')
    if value.lower().startswith("url("):
        value = value[4:].strip().strip('"\'')
    return value.rstrip(".,;)]}").strip()


def _url_identity(url: str) -> tuple[object, ...]:
    parsed = urlparse(url)
    if parsed.scheme.lower() == "magnet":
        return ("magnet", parsed.query.lower())
    hostname = (parsed.hostname or "").lower()
    port = parsed.port
    if (parsed.scheme.lower(), port) in {("http", 80), ("https", 443), ("ftp", 21), ("ftps", 990), ("sftp", 22)}:
        port = None
    query = tuple(sorted(parse_qsl(parsed.query, keep_blank_values=True)))
    return parsed.scheme.lower(), hostname, port, unquote(parsed.path), query


def _path_name(url: str) -> str:
    parsed = urlparse(url)
    return unquote(parsed.path).replace("\\", "/").rsplit("/", 1)[-1]


def _query_names(url: str) -> list[str]:
    parsed = urlparse(url)
    names: list[str] = []
    for key, value in parse_qsl(parsed.query, keep_blank_values=True):
        if key.lower() in _QUERY_NAME_KEYS and value:
            names.append(unquote(value))
    return names


def _extension_of(url: str, download_attr: str = "") -> str:
    for candidate in (download_attr, *_query_names(url), _path_name(url)):
        name = str(candidate or "").replace("\\", "/").rsplit("/", 1)[-1].split("?", 1)[0]
        if "." not in name:
            continue
        ext = name.rsplit(".", 1)[-1].lower()
        if ext and len(ext) <= 8 and ext.isalnum():
            return ext
    return ""

def _filename_for(url: str, download_attr: str = "", link_text: str = "") -> str:
    candidates = [download_attr, *_query_names(url), _path_name(url), link_text]
    for candidate in candidates:
        leaf = html.unescape(unquote(str(candidate or ""))).replace("\\", "/").rsplit("/", 1)[-1]
        leaf = leaf.split("?", 1)[0].split("#", 1)[0].strip()
        if not leaf:
            continue
        cleaned = sanitize_filename(leaf)
        if cleaned and cleaned != "download":
            return cleaned
    ext = _extension_of(url, download_attr)
    return f"download.{ext}" if ext else "download"


def _normalized_harvest_url(raw_value: str, base_url: str) -> str | None:
    raw_value = _clean_raw_url(raw_value)
    if not raw_value or any(char in raw_value for char in '\r\n\t "\'<>\\`{}'):
        return None
    if _PLACEHOLDER_PATTERN.search(raw_value):
        return None
    lowered = raw_value.lower()
    if lowered.startswith(("javascript:", "data:", "blob:", "file:", "mailto:", "tel:", "about:")):
        return None
    if lowered.startswith("magnet:"):
        return raw_value if re.search(r"[?&]xt=", raw_value, re.IGNORECASE) else None
    candidate_url = urljoin(base_url, raw_value)
    try:
        parsed = urlparse(candidate_url)
        hostname = parsed.hostname
        _ = parsed.port
    except ValueError:
        return None
    scheme = parsed.scheme.lower()
    if scheme not in {"http", "https", "ftp", "ftps", "sftp"} or not hostname:
        return None
    if scheme in {"http", "https"} and (parsed.username or parsed.password):
        return None
    if _PLACEHOLDER_PATTERN.search(unquote(parsed.path)):
        return None
    normalized = urlunparse(parsed._replace(fragment=""))
    try:
        ensure_url_allowed(normalized)
    except ValueError:
        return None
    return normalized


def _label_for(filename: str, link_text: str, url: str) -> str:
    text = re.sub(r"\s+", " ", html.unescape(str(link_text or ""))).strip()
    if text and text.lower() not in {url.lower(), filename.lower()} and len(text) <= 200:
        return text
    return filename

class _HarvestParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.title_parts: list[str] = []
        self.records: list[tuple[str, str, str, str]] = []
        self._in_title = False
        self._anchor: dict[str, object] | None = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if len(self.records) >= MAX_RAW_LINKS:
            return
        mapping = {str(key).lower(): value for key, value in attrs if value}
        if tag == "title":
            self._in_title = True
        if tag == "a":
            self._anchor = {
                "href": mapping.get("href", "") or "",
                "download": mapping.get("download", "") or "",
                "text": [],
            }
        if tag != "a":
            source = "href"
            raw = mapping.get("href") or mapping.get("data-href") or mapping.get("data-url") or mapping.get("data-download")
            if not raw:
                raw = mapping.get("src") or mapping.get("data-src")
                source = "src"
            if raw:
                self.records.append((raw, source, mapping.get("download", "") or "", ""))
            if tag in {"source", "video", "audio"} and mapping.get("src"):
                self.records.append((mapping["src"], "src", "", ""))

    def handle_data(self, data: str) -> None:
        if self._in_title:
            self.title_parts.append(data)
        if self._anchor is not None:
            texts = self._anchor["text"]
            assert isinstance(texts, list)
            texts.append(data)

    def handle_endtag(self, tag: str) -> None:
        if tag == "title":
            self._in_title = False
        if tag == "a" and self._anchor is not None:
            href = str(self._anchor.get("href") or "")
            download = str(self._anchor.get("download") or "")
            texts = self._anchor.get("text")
            text = "".join(texts) if isinstance(texts, list) else ""
            if href:
                self.records.append((href, "href", download, text))
            self._anchor = None

def extract_page_links(
    text: str,
    base_url: str,
    extensions: list[str] | None = None,
    limit: int = MAX_HARVEST_LINKS,
) -> tuple[list[HarvestLink], str, bool]:
    allowed = normalize_harvest_extensions(extensions)
    limit = min(max(limit, 0), MAX_HARVEST_LINKS)
    parser = _HarvestParser()
    try:
        parser.feed(text)
    except Exception:
        pass

    raw_records: list[tuple[str, str, str, str]] = list(parser.records)
    decoded = _decode_url_escapes(text)
    for match in _ABS_URL_PATTERN.finditer(decoded):
        raw_records.append((match.group(0), "text", "", ""))
        if len(raw_records) >= MAX_RAW_LINKS:
            break

    links: list[HarvestLink] = []
    seen: set[tuple[object, ...]] = set()
    truncated = False
    for raw_value, source, download_attr, link_text in raw_records:
        candidate = _normalized_harvest_url(raw_value, base_url)
        if not candidate:
            continue
        identity = _url_identity(candidate)
        if identity in seen:
            continue
        lowered = candidate.lower()
        if lowered.startswith("magnet:"):
            extension = ""
            category = "torrent"
        else:
            extension = _extension_of(candidate, download_attr)
            if extension not in allowed:
                continue
            category = harvest_category(extension, candidate)
        seen.add(identity)
        if len(links) >= limit:
            truncated = True
            break
        filename = _filename_for(candidate, download_attr, link_text)
        links.append(HarvestLink(
            url=candidate,
            filename=filename,
            label=_label_for(filename, link_text, candidate),
            extension=extension,
            category=category,
            source=source if source in {"href", "src", "text"} else "href",
        ))
    title = re.sub(r"\s+", " ", "".join(parser.title_parts)).strip()[:_TITLE_LIMIT]
    return links, title, truncated

def _direct_kind(content_type: str, disposition: str, final_url: str, signature: str) -> str:
    if signature.startswith("#EXTM3U"):
        return "hls"
    if _is_dash_manifest(signature, content_type):
        return "dash"
    if _is_direct_file_response(content_type, disposition, final_url):
        return "file"
    return ""


async def harvest_page(
    url: str,
    headers: dict[str, str],
    extensions: list[str] | None = None,
    client=None,
) -> HarvestResult:
    parsed = urlparse(url)
    if parsed.scheme not in {"http", "https"} or not parsed.netloc:
        raise HarvestError("\u9875\u9762\u6293\u53d6\u53ea\u652f\u6301 HTTP(S) \u7f51\u9875\u5730\u5740")

    owned_client = client is None
    http = client or policy_httpx_client(
        follow_redirects=True,
        timeout=httpx.Timeout(15, connect=10),
    )
    try:
        try:
            async with http.stream("GET", url, headers=headers) as response:
                response.raise_for_status()
                final_url = str(response.url)
                ensure_url_allowed(final_url)
                content_type = response.headers.get("content-type", "")
                disposition = response.headers.get("content-disposition", "")
                if _is_direct_file_response(content_type, disposition, final_url) and not (
                    "octet-stream" in content_type.lower()
                    and not urlparse(final_url).path.lower().endswith(tuple(f".{ext}" for ext in DEFAULT_EXTENSIONS))
                    and not disposition
                ):
                    return HarvestResult(
                        kind="file",
                        page_url=url,
                        final_url=final_url,
                        links=[],
                        message=_NOT_PAGE_MESSAGES["file"],
                    )
                body = bytearray()
                async for chunk in response.aiter_bytes():
                    body.extend(chunk)
                    if "octet-stream" in content_type.lower() and len(body) >= 64 * 1024:
                        break
                    if len(body) > MAX_RESPONSE_BYTES:
                        raise HarvestError("\u9875\u9762\u8d85\u8fc7 4 MiB \u8bc6\u522b\u4e0a\u9650")
                encoding = response.encoding or "utf-8"
        except httpx.HTTPStatusError as exc:
            raise HarvestError(f"\u94fe\u63a5\u8bf7\u6c42\u5931\u8d25\uff1aHTTP {exc.response.status_code}") from exc
        except httpx.HTTPError as exc:
            raise HarvestError(f"\u94fe\u63a5\u8bf7\u6c42\u5931\u8d25\uff1a{type(exc).__name__}") from exc

        text = bytes(body).decode(encoding, errors="replace")
        signature = text.lstrip("\ufeff \t\r\n")
        kind = _direct_kind(content_type, disposition, final_url, signature)
        if kind:
            return HarvestResult(
                kind=kind,  # type: ignore[arg-type]
                page_url=url,
                final_url=final_url,
                links=[],
                message=_NOT_PAGE_MESSAGES[kind],
            )

        links, title, truncated = extract_page_links(text, final_url, extensions)
        if links:
            suffix = "\uff0c\u5df2\u622a\u53d6\u524d 100 \u6761" if truncated else ""
            message = f"\u4ece\u5f53\u524d\u9875\u9762\u63d0\u53d6\u5230 {len(links)} \u4e2a\u53ef\u4e0b\u8f7d\u94fe\u63a5{suffix}"
            return HarvestResult(
                kind="page",
                page_url=url,
                final_url=final_url,
                title=title,
                links=links,
                truncated=truncated,
                message=message,
            )
        return HarvestResult(
            kind="none",
            page_url=url,
            final_url=final_url,
            title=title,
            links=[],
            message="\u9875\u9762\u672a\u53d1\u73b0\u53ef\u4e0b\u8f7d\u7684\u9759\u6001\u6587\u4ef6\u94fe\u63a5\u3002\u53ea\u8bfb\u53d6\u5f53\u524d\u8fd9\u4e00\u9875\u7684 HTML\uff0c\u4e0d\u4f1a\u6267\u884c\u811a\u672c\u6216\u7ee7\u7eed\u6253\u5f00\u5b50\u9875\u9762\u3002",
        )
    finally:
        if owned_client:
            await http.aclose()

PROBE_CONCURRENCY = 4
_CONTENT_RANGE_TOTAL = re.compile(
    r'bytes\s+(?:\d+-\d+|\*)\s*/\s*(?P<total>\d+|\*)',
    re.IGNORECASE,
)


class HarvestProbe(BaseModel):
    url: str
    size: int | None = None
    content_type: str = ""
    ok: bool = False


def _size_from_headers(headers: object, *, allow_content_length: bool) -> tuple[int | None, str]:
    getter = getattr(headers, "get", None)
    values = getter if callable(getter) else (lambda key, default="": default)
    content_type = str(values("content-type", "") or "")
    match = _CONTENT_RANGE_TOTAL.search(str(values("content-range", "") or ""))
    if match and match.group("total") != "*":
        total = int(match.group("total"))
        if total > 0:
            return total, content_type
    if allow_content_length:
        length = str(values("content-length", "") or "").strip()
        if length.isdigit() and int(length) > 0:
            return int(length), content_type
    return None, content_type


async def _probe_one(url: str, headers: dict[str, str], client) -> HarvestProbe:
    lowered = str(url or "").lower()
    if lowered.startswith("magnet:"):
        return HarvestProbe(url=url, ok=False)
    parsed = urlparse(url)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        return HarvestProbe(url=url, ok=False)
    try:
        ensure_url_allowed(url)
    except ValueError:
        return HarvestProbe(url=url, ok=False)
    try:
        head = await client.head(url, headers=headers)
        size, content_type = _size_from_headers(head.headers, allow_content_length=True)
        if size:
            return HarvestProbe(url=url, size=size, content_type=content_type, ok=True)
        range_headers = dict(headers)
        range_headers["range"] = "bytes=0-0"
        async with client.stream("GET", url, headers=range_headers) as response:
            size, content_type = _size_from_headers(response.headers, allow_content_length=False)
            return HarvestProbe(url=url, size=size, content_type=content_type, ok=bool(size))
    except Exception:
        return HarvestProbe(url=url, ok=False)


async def probe_harvest_links(
    urls: list[str],
    headers: dict[str, str],
    client=None,
) -> list[HarvestProbe]:
    cleaned: list[str] = []
    seen: set[str] = set()
    for item in urls:
        url = str(item or "").strip()
        if not url or url.lower() in seen:
            continue
        seen.add(url.lower())
        cleaned.append(url)
        if len(cleaned) >= MAX_HARVEST_LINKS:
            break
    if not cleaned:
        return []

    owned_client = client is None
    http = client or policy_httpx_client(
        follow_redirects=True,
        timeout=httpx.Timeout(6, connect=4),
    )
    semaphore = asyncio.Semaphore(PROBE_CONCURRENCY)

    async def run(url: str) -> HarvestProbe:
        async with semaphore:
            return await _probe_one(url, headers, http)

    try:
        return list(await asyncio.gather(*(run(url) for url in cleaned)))
    finally:
        if owned_client:
            await http.aclose()

