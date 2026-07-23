from dataclasses import dataclass
from urllib.parse import parse_qs, urlsplit, urlunsplit

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


def _response_headers(response) -> dict[str, str]:
    raw = getattr(response, "headers", None) or {}
    try:
        items = dict(raw).items()
    except Exception:
        try:
            items = raw.items()
        except Exception:
            return {}
    return {str(name).lower(): str(value) for name, value in items}


def _response_snippet(response, limit: int = 800) -> str:
    for attr in ("text", "content"):
        value = getattr(response, attr, None)
        if value is None:
            continue
        try:
            if isinstance(value, bytes):
                return value[:limit].decode("utf-8", errors="ignore")
            return str(value)[:limit]
        except Exception:
            continue
    return ""


def _looks_like_signed_url(url: str) -> bool:
    if not url:
        return False
    try:
        parsed = urlsplit(url)
        query = parse_qs(parsed.query, keep_blank_values=True)
    except ValueError:
        return False
    keys = {str(key).lower() for key in query}
    markers = {
        "token", "sig", "signature", "expires", "expire", "exp", "key",
        "auth", "authorization", "hdnts", "policy", "x-amz-signature",
        "x-amz-credential", "x-amz-expires", "x-amz-date", "x-amz-security-token",
        "verify", "hash", "jwt",
    }
    if keys & markers:
        return True
    joined = "&".join(keys)
    return "x-amz-" in joined or "signature" in joined


def _looks_like_cloudflare(headers: dict[str, str], body: str) -> bool:
    server = headers.get("server", "").lower()
    if "cloudflare" in server or headers.get("cf-ray"):
        return True
    sample = body.lower()
    needles = (
        "cf-browser-verification",
        "cf-challenge",
        "attention required",
        "just a moment",
        "cloudflare",
        "checking your browser",
        "turnstile",
    )
    return any(item in sample for item in needles)


def _http_hint(
    status: int,
    task_context=None,
    *,
    response=None,
    request_url: str = "",
) -> str:
    headers = {
        str(name).lower(): str(value)
        for name, value in dict(getattr(task_context, "request_headers", {}) or {}).items()
    }
    has_browser_context = bool(
        headers
        or getattr(task_context, "cookie", "")
        or getattr(task_context, "referer", "")
        or getattr(task_context, "origin", "")
    )
    has_credentials = bool(getattr(task_context, "cookie", "") or headers.get("authorization"))
    response_headers = _response_headers(response) if response is not None else {}
    body = _response_snippet(response) if response is not None else ""
    signed = _looks_like_signed_url(request_url or getattr(task_context, "url", "") or "")
    cloudflare = _looks_like_cloudflare(response_headers, body)
    has_referer = bool(getattr(task_context, "referer", "") or headers.get("referer"))

    if status == 401:
        if has_credentials:
            return "登录凭据或授权令牌已失效。回到原网页刷新并重新识别后再下载；重复点击重试不会更新令牌。"
        return "资源需要登录或授权。请从原网页用浏览器扩展重新发送，并在扩展面板授权本页 Cookie。"

    if status == 403:
        parts: list[str] = []
        if cloudflare:
            parts.append("响应像是 Cloudflare / 人机验证拦截，直接重试旧任务通常无效。")
        if signed:
            parts.append("链接含签名/过期参数，签名 URL 过期后必须回到原网页重新获取。")
        if not has_referer and not has_browser_context:
            parts.append("资源缺少 Referer/Origin/Cookie 等网页请求上下文，常见于防盗链。")
        elif has_browser_context or has_credentials:
            parts.append("已携带网页上下文仍被拒绝，多半是会话、签名或临时令牌过期。")
        if not parts:
            parts.append("服务器拒绝访问该资源。")
        parts.append("处理：用浏览器扩展从原页面重新发送；需要登录时先授权本页 Cookie；不要套用其他站点的 Referer/Origin；降低并发后重试。")
        return "".join(parts)

    if status == 407:
        return "代理服务器要求认证。检查系统/网络代理的账号密码，或临时关闭 VPN、代理和 HTTPS 检查后再试。"
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
    task_context=None,
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
        headers = _response_headers(response)
        body = _response_snippet(response)
        code = f"HTTP_{status}"
        if status == 403 and _looks_like_cloudflare(headers, body):
            code = "HTTP_403_CLOUDFLARE"
        elif status == 403 and _looks_like_signed_url(request_url):
            code = "HTTP_403_EXPIRED_SIGNATURE"
        return DownloadErrorDetails(
            code=code,
            message=f"HTTP {status} {reason}",
            hint=_http_hint(status, task_context, response=response, request_url=request_url),
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
    task_context=None,
) -> DownloadError:
    if isinstance(exc, DownloadError):
        return exc
    return DownloadError(
        diagnose_download_error(
            exc,
            stage=stage,
            url=url,
            attempt=attempt,
            task_context=task_context,
        )
    )


def http_status_from_exception(exc: BaseException) -> int:
    """Return an HTTP status from httpx/curl transports, including wrapped errors."""
    for item in _exception_chain(exc):
        status = int(getattr(getattr(item, "response", None), "status_code", 0) or 0)
        if status:
            return status
    return 0


def should_retry_download_error(exc: BaseException) -> bool:
    """Retry transient failures, but never hammer stale/authenticated URLs."""
    status = http_status_from_exception(exc)
    if not status:
        return True
    return status in {408, 425, 429} or 500 <= status <= 599
