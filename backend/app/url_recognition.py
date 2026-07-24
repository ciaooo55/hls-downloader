import html
import re
from dataclasses import dataclass
from html.parser import HTMLParser
from typing import Literal
from urllib.parse import parse_qsl, unquote, urljoin, urlparse, urlunparse

import httpx
from pydantic import BaseModel


class RecognitionError(RuntimeError):
    pass


class HlsCandidate(BaseModel):
    url: str
    source: str = ""
    # Defaults keep older API consumers working while giving the UI something
    # useful to display instead of a long signed URL.
    label: str = ""
    quality: str | None = None
    confidence: float = 0.0


class RecognitionResult(BaseModel):
    kind: Literal["hls", "file", "page", "none"]
    final_url: str
    candidates: list[HlsCandidate]
    message: str = ""


MAX_RESPONSE_BYTES = 4 * 1024 * 1024
# Inspect many raw matches, but never flood the chooser with all of them.
MAX_CANDIDATES = 12
MAX_RAW_CANDIDATES = 512

_SCRIPT_HLS_PATTERN = re.compile(
    r"(?P<url>(?:(?:https?:)?//|\.\.?/|/)[^\s\"'<>\\]+?\.m3u8"
    r"(?=\?|[\s\"'<>\\,;\)\]}]|$)(?:\?[^\s\"'<>\\]*)?)",
    re.IGNORECASE,
)
_QUOTED_HLS_PATTERN = re.compile(
    r"(?P<quote>[\"'])(?P<url>[^\"'<>\r\n]*?\.m3u8(?:\?[^\"'<>\r\n]*)?)(?P=quote)",
    re.IGNORECASE,
)
_DOWNLOAD_EXTENSIONS = re.compile(
    r"\.(?:zip|7z|rar|tar|gz|bz2|xz|iso|exe|msi|dmg|apk|pdf|mp4|mkv|webm|mov|avi|mp3|m4a|flac|wav|torrent)$",
    re.IGNORECASE,
)
_PLACEHOLDER_PATTERN = re.compile(
    r"(?:\$\{|\{\{|\}\}|<%|%7b|%7d|\[object(?:%20|\s)+object\]|(?:^|[/_.-])(?:undefined|null)(?:[/_.?-]|$))",
    re.IGNORECASE,
)
_NON_VIDEO_PATH_PATTERN = re.compile(
    r"(?:^|[/_.-])(?:"
    r"audio(?:only)?|audiotrack|subtitle(?:s)?|subtitles?|captions?|"
    r"thumbnail(?:s)?|thumbs?|sprite(?:s)?|storyboard(?:s)?|preview(?:s)?|"
    r"iframe|trickplay|ads?|advert(?:s|ising)?|preroll|midroll|postroll"
    r")(?:[/_.-]|$)",
    re.IGNORECASE,
)
_MASTER_PATTERN = re.compile(
    r"(?:^|[/_.-])(?:master|multivariant|multi-?variant|adaptive|auto)(?:[/_.-]|$)",
    re.IGNORECASE,
)
_DIMENSION_PATTERN = re.compile(r"(?<!\d)(?:\d{3,4})[xX](?P<height>\d{3,4})(?!\d)")
_RESOLUTION_PATTERN = re.compile(
    r"(?<!\d)(?P<height>2160|1440|1200|1080|900|720|576|540|480|432|360|288|270|240|180|144)p?(?!\d)",
    re.IGNORECASE,
)
_BITRATE_PATTERN = re.compile(r"(?<!\d)(?P<bitrate>\d{2,5})\s*k(?:bps)?(?![a-z])", re.IGNORECASE)
_QUALITY_ALIAS_PATTERN = re.compile(
    r"(?<![a-z0-9])(?P<alias>8k|4k|uhd|2k|qhd|fhd|full-?hd|hd|sd|high|medium|low)(?![a-z0-9])",
    re.IGNORECASE,
)
_QUALITY_ALIAS_HEIGHTS = {
    "8k": 4320, "4k": 2160, "uhd": 2160, "2k": 1440, "qhd": 1440,
    "fhd": 1080, "fullhd": 1080, "full-hd": 1080, "high": 1080,
    "hd": 720, "medium": 720, "sd": 480, "low": 360,
}

_VOLATILE_QUERY_KEYS = {
    "access-token", "accesstoken", "auth", "auth-token", "authorization",
    "e", "exp", "expiration", "expires", "expires-at", "expiry",
    "hmac", "hash", "hdnea", "hdnts", "jwt", "key-pair-id", "keypairid",
    "nonce", "policy", "secure", "session", "session-id", "sessionid",
    "sig", "signature", "st", "timestamp", "token", "ts", "verify",
    "wssecret", "wstime",
}
_QUALITY_QUERY_KEYS = {
    "bandwidth", "bitrate", "br", "bw", "height", "quality", "q",
    "rendition", "res", "resolution", "width",
}


def _is_direct_file_response(content_type: str, disposition: str, final_url: str) -> bool:
    mime = content_type.split(";", 1)[0].strip().lower()
    if re.search(r"(?:^|;)\s*attachment(?:;|$)", disposition, re.IGNORECASE):
        return True
    if _DOWNLOAD_EXTENSIONS.search(urlparse(final_url).path):
        return True
    if mime.startswith(("video/", "audio/", "image/", "font/")):
        return True
    if mime in {
        "application/octet-stream", "application/zip", "application/x-7z-compressed",
        "application/x-rar-compressed", "application/x-bittorrent", "application/pdf",
        "application/x-iso9660-image", "application/vnd.microsoft.portable-executable",
    }:
        return True
    return bool(mime and not (
        mime.startswith("text/") or "html" in mime or "xml" in mime
        or "json" in mime or "mpegurl" in mime
    ))


class _CandidateParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.values: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if len(self.values) >= MAX_RAW_CANDIDATES:
            return
        for _, value in attrs:
            if value and ".m3u8" in value.lower():
                self.values.append(value)
                if len(self.values) >= MAX_RAW_CANDIDATES:
                    return


def _decode_url_escapes(value: str) -> str:
    """Decode only escapes commonly used around URLs, not arbitrary JS."""
    value = html.unescape(value).replace("\\/", "/")
    replacements = {
        "\\u0026": "&", "\\u002f": "/", "\\u003a": ":", "\\u003d": "=",
        "\\x26": "&", "\\x2f": "/", "\\x3a": ":", "\\x3d": "=",
    }
    for escaped, decoded in replacements.items():
        value = re.sub(re.escape(escaped), decoded, value, flags=re.IGNORECASE)
    return value


def _clean_raw_url(value: str) -> str:
    value = _decode_url_escapes(value).strip().strip("\"'")
    if value.lower().startswith("url("):
        value = value[4:].strip().strip("\"'")
    return value.rstrip(".,;)]}").strip()


def _normalized_candidate_url(raw_value: str, base_url: str) -> str | None:
    raw_value = _clean_raw_url(raw_value)
    if not raw_value or any(char in raw_value for char in "\r\n\t \"'<>\\`{}|^"):
        return None
    if _PLACEHOLDER_PATTERN.search(raw_value):
        return None
    candidate_url = urljoin(base_url, raw_value)
    try:
        parsed = urlparse(candidate_url)
        hostname = parsed.hostname
        _ = parsed.port
    except ValueError:
        return None
    if parsed.scheme.lower() not in {"http", "https"} or not hostname:
        return None
    if parsed.username or parsed.password:
        return None
    decoded_path = unquote(parsed.path)
    if (
        not decoded_path.lower().endswith(".m3u8")
        or decoded_path.rsplit("/", 1)[-1].lower() == ".m3u8"
    ):
        return None
    searchable = f"{decoded_path}?{unquote(parsed.query)}"
    if _PLACEHOLDER_PATTERN.search(searchable) or _NON_VIDEO_PATH_PATTERN.search(searchable):
        return None
    try:
        query_pairs = parse_qsl(parsed.query, keep_blank_values=True)
    except ValueError:
        return None
    for key, value in query_pairs:
        if key.lower() in {"type", "track", "kind", "media"} and value.lower() in {
            "audio", "subtitle", "subtitles", "caption", "captions", "iframe", "ad",
        }:
            return None
    return urlunparse(parsed._replace(fragment=""))


def _normalized_query_key(key: str) -> str:
    return key.strip().lower().replace("_", "-")


def _is_volatile_query_key(key: str) -> bool:
    key = _normalized_query_key(key)
    return (
        key in _VOLATILE_QUERY_KEYS
        or key.startswith(("x-amz-", "x-goog-", "cloudfront-"))
        or key.endswith(("-signature", "-token"))
    )


def _url_identity(url: str) -> tuple[object, ...]:
    parsed = urlparse(url)
    hostname = (parsed.hostname or "").lower()
    port = parsed.port
    if (parsed.scheme.lower(), port) in {("http", 80), ("https", 443)}:
        port = None
    stable_query = tuple(sorted(
        (_normalized_query_key(key), value)
        for key, value in parse_qsl(parsed.query, keep_blank_values=True)
        if not _is_volatile_query_key(key)
    ))
    return parsed.scheme.lower(), hostname, port, unquote(parsed.path), stable_query


def _quality_details(url: str) -> tuple[int | None, int | None]:
    parsed = urlparse(url)
    decoded = unquote(f"{parsed.path}?{parsed.query}")
    dimensions = [int(match.group("height")) for match in _DIMENSION_PATTERN.finditer(decoded)]
    if dimensions:
        return max(dimensions), None

    # Explicit quality parameters avoid mistaking an unrelated ID elsewhere in
    # the URL for a resolution.
    for key, value in parse_qsl(parsed.query, keep_blank_values=True):
        if _normalized_query_key(key) in _QUALITY_QUERY_KEYS:
            dimension = _DIMENSION_PATTERN.search(value)
            resolution = _RESOLUTION_PATTERN.search(value)
            if dimension:
                return int(dimension.group("height")), None
            if resolution:
                return int(resolution.group("height")), None
            alias = _QUALITY_ALIAS_PATTERN.search(value)
            if alias:
                return _QUALITY_ALIAS_HEIGHTS[alias.group("alias").lower()], None
            if value.isdigit() and _normalized_query_key(key) in {"bandwidth", "bitrate", "br", "bw"}:
                bitrate = int(value)
                return None, bitrate // 1000 if bitrate >= 100_000 else bitrate

    resolutions = [int(match.group("height")) for match in _RESOLUTION_PATTERN.finditer(unquote(parsed.path))]
    if resolutions:
        return max(resolutions), None
    aliases = [
        _QUALITY_ALIAS_HEIGHTS[match.group("alias").lower()]
        for match in _QUALITY_ALIAS_PATTERN.finditer(unquote(parsed.path))
    ]
    if aliases:
        return max(aliases), None
    bitrates = [int(match.group("bitrate")) for match in _BITRATE_PATTERN.finditer(decoded)]
    return None, max(bitrates) if bitrates else None


def _is_master_url(url: str) -> bool:
    parsed = urlparse(url)
    decoded = unquote(f"{parsed.path}?{parsed.query}")
    if _MASTER_PATTERN.search(decoded):
        return True
    return any(
        _normalized_query_key(key) in {"type", "playlist", "manifest"}
        and value.lower() in {"master", "multivariant", "adaptive"}
        for key, value in parse_qsl(parsed.query, keep_blank_values=True)
    )


def _family_path(path: str) -> str:
    value = unquote(path).lower()
    if value.endswith(".m3u8"):
        value = value[:-5]
    value = _DIMENSION_PATTERN.sub("-", value)
    value = _RESOLUTION_PATTERN.sub("-", value)
    value = _BITRATE_PATTERN.sub("-", value)
    value = _QUALITY_ALIAS_PATTERN.sub("-", value)
    value = re.sub(
        r"(?:^|[/_.-])(?:master|multivariant|multi-?variant|adaptive|auto)(?=$|[/_.-])",
        "-",
        value,
    )
    value = re.sub(r"[-_.]+", "-", value)
    value = re.sub(r"[-_.]+(?=/|$)", "", value)
    value = re.sub(r"(?:^|/)(?:index|playlist|manifest|stream|video)(?=$|/)", "/", value)
    value = re.sub(r"/+", "/", value)
    return value.rstrip("/-_.") or "/"


def _family_identity(url: str) -> tuple[object, ...]:
    parsed = urlparse(url)
    hostname = (parsed.hostname or "").lower()
    port = parsed.port
    if (parsed.scheme.lower(), port) in {("http", 80), ("https", 443)}:
        port = None
    family_query = tuple(sorted(
        (_normalized_query_key(key), value)
        for key, value in parse_qsl(parsed.query, keep_blank_values=True)
        if not _is_volatile_query_key(key)
        and _normalized_query_key(key) not in _QUALITY_QUERY_KEYS
    ))
    return parsed.scheme.lower(), hostname, port, _family_path(parsed.path), family_query


@dataclass(frozen=True)
class _RankedCandidate:
    candidate: HlsCandidate
    family: tuple[object, ...]
    is_master: bool
    height: int | None
    bitrate: int | None
    order: int

    @property
    def rank(self) -> tuple[int, int, int, float, int]:
        # A master lets the downloader select renditions itself, so it wins over
        # any one media rendition. Otherwise retain the best known quality.
        return (
            1 if self.is_master else 0,
            self.height or 0,
            self.bitrate or 0,
            self.candidate.confidence,
            -self.order,
        )


def _rank_html_candidate(url: str, origin: str, order: int) -> _RankedCandidate:
    is_master = _is_master_url(url)
    height, bitrate = _quality_details(url)
    confidence = 0.92 if origin == "attribute" else 0.82
    if is_master:
        confidence += 0.06
    elif height or bitrate:
        confidence += 0.03
    confidence = round(min(confidence, 0.99), 2)
    if is_master:
        label, quality = "主播放清单", "master"
    elif height:
        label = quality = f"{height}p"
    elif bitrate:
        label, quality = f"{bitrate} kbps", f"{bitrate}kbps"
    else:
        label, quality = "HLS 播放清单", None
    candidate = HlsCandidate(
        url=url,
        source="html",
        label=label,
        quality=quality,
        confidence=confidence,
    )
    return _RankedCandidate(candidate, _family_identity(url), is_master, height, bitrate, order)


def _attribute_matches(value: str) -> list[str]:
    value = _decode_url_escapes(value)
    matches = [value]
    matches.extend(match.group("url") for match in _SCRIPT_HLS_PATTERN.finditer(value))
    matches.extend(match.group("url") for match in _QUOTED_HLS_PATTERN.finditer(value))
    # srcset-like attributes otherwise leave a density suffix on the URL.
    matches.extend(part for part in re.split(r"[\s,]+", value) if ".m3u8" in part.lower())
    # Direct attributes are also found by the regexes; keep their first form so
    # they do not consume the raw inspection budget three times over.
    return list(dict.fromkeys(matches))


def _is_concatenated_match(text: str, start: int, end: int) -> bool:
    """Reject partial URLs taken from a JavaScript string concatenation."""
    before = text[max(0, start - 24):start].rstrip().rstrip("\"'").rstrip()
    after = text[end:end + 24].lstrip().lstrip("\"'").lstrip()
    return before.endswith("+") or after.startswith("+")


def extract_html_candidates(text: str, base_url: str, limit: int = MAX_CANDIDATES) -> list[HlsCandidate]:
    limit = min(limit, MAX_CANDIDATES)
    if limit <= 0:
        return []
    parser = _CandidateParser()
    try:
        parser.feed(text)
    except Exception:
        pass

    raw_values: list[tuple[str, str]] = []

    def add(value: str, origin: str) -> None:
        if len(raw_values) < MAX_RAW_CANDIDATES:
            raw_values.append((value, origin))

    for attribute in parser.values:
        for value in _attribute_matches(attribute):
            add(value, "attribute")

    normalized = _decode_url_escapes(text)
    for match in _QUOTED_HLS_PATTERN.finditer(normalized):
        if not _is_concatenated_match(normalized, match.start(), match.end()):
            add(match.group("url"), "script")
    for match in _SCRIPT_HLS_PATTERN.finditer(normalized):
        if not _is_concatenated_match(normalized, match.start(), match.end()):
            add(match.group("url"), "script")

    deduplicated: list[tuple[str, str]] = []
    seen: set[tuple[object, ...]] = set()
    for raw_value, origin in raw_values:
        candidate_url = _normalized_candidate_url(raw_value, base_url)
        if not candidate_url:
            continue
        identity = _url_identity(candidate_url)
        if identity in seen:
            continue
        seen.add(identity)
        deduplicated.append((candidate_url, origin))

    family_order: list[tuple[object, ...]] = []
    families: dict[tuple[object, ...], _RankedCandidate] = {}
    for order, (candidate_url, origin) in enumerate(deduplicated):
        ranked = _rank_html_candidate(candidate_url, origin, order)
        current = families.get(ranked.family)
        if current is None:
            family_order.append(ranked.family)
            families[ranked.family] = ranked
        elif ranked.rank > current.rank:
            families[ranked.family] = ranked
    return [families[family].candidate for family in family_order[:limit]]


def _direct_candidate(url: str, source: str, playlist_text: str = "") -> HlsCandidate:
    is_master = source == "playlist" and (
        "#EXT-X-STREAM-INF" in playlist_text.upper() or _is_master_url(url)
    )
    height, bitrate = _quality_details(url)
    if is_master:
        label, quality = "主播放清单", "master"
    elif height:
        label = quality = f"{height}p"
    elif bitrate:
        label, quality = f"{bitrate} kbps", f"{bitrate}kbps"
    elif source == "file":
        label, quality = "直接媒体文件", None
    else:
        label, quality = "HLS 播放清单", None
    return HlsCandidate(
        url=url,
        source=source,
        label=label,
        quality=quality,
        confidence=1.0,
    )


async def recognize_url(url: str, headers: dict[str, str], client=None) -> RecognitionResult:
    parsed = urlparse(url)
    if parsed.scheme not in {"http", "https"} or not parsed.netloc:
        raise RecognitionError("链接必须是有效的 HTTP(S) 地址")

    owned_client = client is None
    http = client or httpx.AsyncClient(follow_redirects=True, timeout=15)
    try:
        try:
            async with http.stream("GET", url, headers=headers) as response:
                response.raise_for_status()
                final_url = str(response.url)
                content_type = response.headers.get("content-type", "")
                disposition = response.headers.get("content-disposition", "")
                # Do not read multi-gigabyte archives or installers into the page recognizer.
                # Extensionless octet-stream responses remain sniffed below so HLS signatures
                # served with a generic MIME type are still recognized correctly.
                if _is_direct_file_response(content_type, disposition, final_url) and not (
                    "octet-stream" in content_type.lower()
                    and not _DOWNLOAD_EXTENSIONS.search(urlparse(final_url).path)
                    and not disposition
                ):
                    return RecognitionResult(
                        kind="file",
                        final_url=final_url,
                        candidates=[_direct_candidate(final_url, "file")],
                    )
                body = bytearray()
                async for chunk in response.aiter_bytes():
                    body.extend(chunk)
                    if "octet-stream" in content_type.lower() and len(body) >= 64 * 1024:
                        break
                    if len(body) > MAX_RESPONSE_BYTES:
                        raise RecognitionError("页面超过 4 MiB 识别上限")
                encoding = response.encoding or "utf-8"
        except httpx.HTTPError as exc:
            raise RecognitionError(f"链接请求失败：{exc}") from exc

        text = bytes(body).decode(encoding, errors="replace")
        signature = text.lstrip("\ufeff \t\r\n")
        if signature.startswith("#EXTM3U"):
            return RecognitionResult(
                kind="hls",
                final_url=final_url,
                candidates=[_direct_candidate(final_url, "playlist", signature)],
            )

        if _is_direct_file_response(content_type, disposition, final_url):
            return RecognitionResult(
                kind="file",
                final_url=final_url,
                candidates=[_direct_candidate(final_url, "file")],
            )

        candidates = extract_html_candidates(text, final_url)
        if candidates:
            return RecognitionResult(kind="page", final_url=final_url, candidates=candidates)
        return RecognitionResult(
            kind="none",
            final_url=final_url,
            candidates=[],
            message="页面未发现静态 HLS。请安装浏览器插件，在原网页播放视频后从插件资源面板发送下载。",
        )
    finally:
        if owned_client:
            await http.aclose()
