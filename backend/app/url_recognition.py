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
    kind: Literal["hls", "page", "none"]
    final_url: str
    candidates: list[HlsCandidate]
    message: str = ""


MAX_RESPONSE_BYTES = 4 * 1024 * 1024
MAX_CANDIDATES = 100
_SCRIPT_HLS_PATTERN = re.compile(
    r"(?P<url>(?:https?:)?//[^\s\"'<>]+?\.m3u8(?:\?[^\s\"'<>]*)?|(?:\.\.?/|/)[^\s\"'<>]+?\.m3u8(?:\?[^\s\"'<>]*)?)",
    re.IGNORECASE,
)


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
                body = bytearray()
                async for chunk in response.aiter_bytes():
                    body.extend(chunk)
                    if len(body) > MAX_RESPONSE_BYTES:
                        raise RecognitionError("页面超过 4 MiB 识别上限")
                final_url = str(response.url)
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

        candidates = extract_html_candidates(text, final_url)
        if candidates:
            return RecognitionResult(kind="page", final_url=final_url, candidates=candidates)
        return RecognitionResult(
            kind="none",
            final_url=final_url,
            candidates=[],
            message="页面未发现静态 HLS，请使用 ScriptCat 或 Tampermonkey 浏览器脚本嗅探动态媒体请求。",
        )
    finally:
        if owned_client:
            await http.aclose()
