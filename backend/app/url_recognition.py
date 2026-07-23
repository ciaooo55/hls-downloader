import re
from html.parser import HTMLParser
from typing import Literal
from urllib.parse import urljoin, urlparse

import httpx
from pydantic import BaseModel


class RecognitionError(RuntimeError):
    pass


class HlsCandidate(BaseModel):
    url: str
    source: str = ""


class RecognitionResult(BaseModel):
    kind: Literal["hls", "file", "page", "none"]
    final_url: str
    candidates: list[HlsCandidate]
    message: str = ""


MAX_RESPONSE_BYTES = 4 * 1024 * 1024
MAX_CANDIDATES = 100
_SCRIPT_HLS_PATTERN = re.compile(
    r"(?P<url>(?:https?:)?//[^\s\"'<>]+?\.m3u8(?:\?[^\s\"'<>]*)?|(?:\.\.?/|/)[^\s\"'<>]+?\.m3u8(?:\?[^\s\"'<>]*)?)",
    re.IGNORECASE,
)
_DOWNLOAD_EXTENSIONS = re.compile(
    r"\.(?:zip|7z|rar|tar|gz|bz2|xz|iso|exe|msi|dmg|apk|pdf|mp4|mkv|webm|mov|avi|mp3|m4a|flac|wav|torrent)$",
    re.IGNORECASE,
)


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
        mime.startswith("text/")
        or "html" in mime
        or "xml" in mime
        or "json" in mime
        or "mpegurl" in mime
    ))


class _CandidateParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.values: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        for _, value in attrs:
            if value and ".m3u8" in value.lower():
                self.values.append(value)


def extract_html_candidates(text: str, base_url: str, limit: int = MAX_CANDIDATES) -> list[HlsCandidate]:
    parser = _CandidateParser()
    try:
        parser.feed(text)
    except Exception:
        pass

    normalized = text.replace("\\/", "/")
    raw_values = parser.values + [match.group("url") for match in _SCRIPT_HLS_PATTERN.finditer(normalized)]
    candidates: list[HlsCandidate] = []
    seen: set[str] = set()
    for raw_value in raw_values:
        candidate_url = urljoin(base_url, raw_value.strip())
        parsed = urlparse(candidate_url)
        if parsed.scheme not in {"http", "https"} or not parsed.netloc:
            continue
        if candidate_url in seen:
            continue
        seen.add(candidate_url)
        candidates.append(HlsCandidate(url=candidate_url, source="html"))
        if len(candidates) >= limit:
            break
    return candidates


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
                        candidates=[HlsCandidate(url=final_url, source="file")],
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
                candidates=[HlsCandidate(url=final_url, source="playlist")],
            )

        if _is_direct_file_response(content_type, disposition, final_url):
            return RecognitionResult(
                kind="file",
                final_url=final_url,
                candidates=[HlsCandidate(url=final_url, source="file")],
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
