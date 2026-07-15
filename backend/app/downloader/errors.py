from dataclasses import dataclass
from urllib.parse import urlsplit, urlunsplit

import httpx
try:
    from curl_cffi.requests.exceptions import (
        ChunkedEncodingError as CurlProtocolError,
        ConnectionError as CurlConnectionError,
        Timeout as CurlTimeout,
    )
except ImportError:
    _CURL_PROTOCOL_ERRORS = ()
    _CURL_CONNECTION_ERRORS = ()
    _CURL_TIMEOUT_ERRORS = ()
else:
    _CURL_PROTOCOL_ERRORS = (CurlProtocolError,)
    _CURL_CONNECTION_ERRORS = (CurlConnectionError,)
    _CURL_TIMEOUT_ERRORS = (CurlTimeout,)


@dataclass(frozen=True)
class DownloadErrorDetails:
    code: str
    message: str
    hint: str
    stage: str = ""
    url: str = ""
    http_status: int = 0
    attempt: int = 0


class DownloadError(RuntimeError):
    def __init__(self, details: DownloadErrorDetails) -> None:
        self.details = details
        super().__init__(format_download_error(details))


def redact_url(value: str) -> str:
    if not value:
        return ""
    try:
        parsed = urlsplit(value)
        if not parsed.scheme or not parsed.netloc:
            return value[:500]
        return urlunsplit((parsed.scheme, parsed.netloc, parsed.path, "", ""))
    except ValueError:
        return value[:500]


def _exception_chain(exc: BaseException):
    seen: set[int] = set()
    current: BaseException | None = exc
    while current is not None and id(current) not in seen:
        seen.add(id(current))
        yield current
        current = current.__cause__ or current.__context__


def _http_hint(status: int) -> str:
    if status in {401, 403}:
        return "网站拒绝访问。检查 Referer、Origin、Cookie 是否正确，确认登录状态和链接尚未过期。"
    if status in {404, 410}:
        return "资源不存在或链接已经过期。请回到视频页面重新获取 m3u8 地址。"
    if status == 429:
        return "请求过于频繁。请降低并发和同时任务数，等待一会后重试。"
    if 500 <= status <= 599:
        return "源站服务器暂时异常。请稍后重试；持续失败时重新获取播放地址。"
    return "服务器返回了异常状态。请检查链接、请求头和网站访问状态。"


def diagnose_download_error(
    exc: BaseException,
    *,
    stage: str = "",
    url: str = "",
    attempt: int = 0,
) -> DownloadErrorDetails:
    chain = list(_exception_chain(exc))
    existing = next((item for item in chain if isinstance(item, DownloadError)), None)
    if existing is not None:
        return existing.details

    http_error = next(
        (
            item
            for item in chain
            if getattr(getattr(item, "response", None), "status_code", 0) >= 400
        ),
        None,
    )
    if http_error is not None:
        response = http_error.response
        status = response.status_code
        reason = (
            getattr(response, "reason_phrase", "")
            or getattr(response, "reason", "")
            or "HTTP error"
        )
        request = getattr(http_error, "request", None)
        request_url = (
            str(request.url)
            if request is not None
            else str(getattr(response, "url", "") or url)
        )
        return DownloadErrorDetails(
            code=f"HTTP_{status}",
            message=f"HTTP {status} {reason}",
            hint=_http_hint(status),
            stage=stage,
            url=redact_url(request_url),
            http_status=status,
            attempt=attempt,
        )

    if any(
        isinstance(item, (httpx.TimeoutException, *_CURL_TIMEOUT_ERRORS))
        for item in chain
    ):
        return DownloadErrorDetails(
            code="NETWORK_TIMEOUT",
            message="连接或读取数据超时",
            hint="检查网络和代理，降低并发后重试；如果只有该网站失败，请重新获取链接。",
            stage=stage,
            url=redact_url(url),
            attempt=attempt,
        )
    if any(
        isinstance(item, (httpx.ConnectError, *_CURL_CONNECTION_ERRORS))
        for item in chain
    ):
        return DownloadErrorDetails(
            code="NETWORK_CONNECT_FAILED",
            message="无法连接到资源服务器",
            hint="检查网络、DNS、防火墙或代理设置，并确认网站当前可以打开。",
            stage=stage,
            url=redact_url(url),
            attempt=attempt,
        )
    if any(
        isinstance(item, (httpx.RemoteProtocolError, *_CURL_PROTOCOL_ERRORS))
        for item in chain
    ):
        return DownloadErrorDetails(
            code="NETWORK_PROTOCOL_ERROR",
            message="服务器提前断开或返回了无效网络响应",
            hint="降低并发后重试；持续发生时通常是源站限制或网络代理不兼容。",
            stage=stage,
            url=redact_url(url),
            attempt=attempt,
        )

    root = chain[-1] if chain else exc
    raw = str(root).strip() or root.__class__.__name__
    lowered = raw.lower()
    if root.__class__.__name__ == "UnsupportedPlaylistError":
        code = "HLS_UNSUPPORTED"
        hint = "该播放列表使用了当前不支持的直播、DRM、SAMPLE-AES 或独立音轨格式。"
    elif "content-range" in lowered or "byterange" in lowered or "range" in lowered:
        code = "HLS_RANGE_INVALID"
        hint = "服务器没有正确支持 Range 请求。重新获取链接，或确认 CDN 没有拦截分段请求。"
    elif "密钥" in raw or "key" in lowered:
        code = "HLS_KEY_INVALID"
        hint = "AES 密钥无效或无法访问。检查 Referer、Origin、Cookie，并重新获取播放地址。"
    elif "解密" in raw or "padding" in lowered or "encrypted" in lowered:
        code = "HLS_DECRYPT_FAILED"
        hint = "分片解密失败。链接、密钥或 IV 可能已过期，请重新获取 m3u8。"
    elif "没有分片" in raw:
        code = "HLS_EMPTY_PLAYLIST"
        hint = "播放列表中没有可下载分片，可能是直播、空清单或地址已失效。"
    elif stage in {"merging", "remuxing", "verifying"} or "ffmpeg" in lowered or "ffprobe" in lowered:
        code = "FFMPEG_MERGE_FAILED"
        hint = "FFmpeg 合并或输出验证失败。查看任务日志，并确认磁盘空间和输出目录可写。"
    elif stage in {"downloading_m3u8", "parsing"}:
        code = "HLS_PLAYLIST_FAILED"
        hint = "无法读取或解析播放列表。检查链接和请求头，必要时从网页重新识别。"
    else:
        code = "DOWNLOAD_FAILED"
        hint = "查看任务日志中的最后一次失败记录，并检查链接、请求头、网络和磁盘空间。"

    return DownloadErrorDetails(
        code=code,
        message=raw[:500],
        hint=hint,
        stage=stage,
        url=redact_url(url),
        attempt=attempt,
    )


def format_download_error(details: DownloadErrorDetails) -> str:
    result = f"[{details.code}] {details.message}"
    if details.attempt:
        result += f"（已尝试 {details.attempt} 次）"
    if details.hint:
        result += f"；建议：{details.hint}"
    return result


def as_download_error(
    exc: BaseException,
    *,
    stage: str,
    url: str = "",
    attempt: int = 0,
) -> DownloadError:
    if isinstance(exc, DownloadError):
        return exc
    return DownloadError(
        diagnose_download_error(exc, stage=stage, url=url, attempt=attempt)
    )
