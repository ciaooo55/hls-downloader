from __future__ import annotations

from pathlib import Path
from urllib.parse import unquote, urlsplit

from .errors import DownloadError, DownloadErrorDetails, redact_url


_HTML_EXTENSIONS = {".htm", ".html", ".xhtml"}
_JSON_EXTENSIONS = {".geojson", ".json", ".jsonl", ".ndjson"}
_STRONG_BINARY_EXTENSIONS = {
    ".7z", ".apk", ".avi", ".bin", ".bz2", ".cab", ".dmg", ".doc", ".docx",
    ".exe", ".flac", ".gz", ".img", ".iso", ".jar", ".m4a", ".mkv", ".mov",
    ".mp3", ".mp4", ".msi", ".pdf", ".ppt", ".pptx", ".rar", ".tar", ".tgz",
    ".torrent", ".wav", ".webm", ".whl", ".xls", ".xlsx", ".xz", ".zip",
}
_MAX_ERROR_DOCUMENT_BYTES = 1024 * 1024


def _suffixes(task, final_url: str, server_filename: str) -> set[str]:
    values = [
        str(getattr(task, "filename", "") or ""),
        server_filename,
        str(getattr(task, "url", "") or ""),
        final_url,
    ]
    result: set[str] = set()
    for value in values:
        if not value:
            continue
        try:
            name = unquote(urlsplit(value).path).rsplit("/", 1)[-1]
        except ValueError:
            name = value.rsplit("/", 1)[-1]
        suffix = Path(name).suffix.lower()
        if suffix:
            result.add(suffix)
    return result


def _looks_like_html(preview: bytes) -> bool:
    sample = preview[:65536].lstrip(b"\xef\xbb\xbf\x00\t\r\n ").lower()
    return (
        sample.startswith((b"<!doctype html", b"<html", b"<head", b"<body"))
        or b"<html" in sample[:2048]
    )


def _looks_like_json(preview: bytes) -> bool:
    sample = preview[:65536].lstrip(b"\xef\xbb\xbf\x00\t\r\n ")
    return sample.startswith((b"{", b"["))


def _looks_like_text_error(preview: bytes) -> bool:
    sample = preview[:4096].decode("utf-8", errors="ignore").strip().lower()
    if not sample:
        return False
    markers = (
        "access denied", "authentication required", "captcha", "error", "forbidden",
        "invalid token", "login required", "not found", "signature expired", "unauthorized",
    )
    return any(marker in sample for marker in markers)


def validate_download_response(
    task,
    *,
    content_type: str = "",
    content_length: int = 0,
    preview: bytes = b"",
    final_url: str = "",
    server_filename: str = "",
) -> None:
    """Reject successful HTTP responses that are actually login/error documents.

    The check deliberately uses the expected filename/MIME as context. Explicit
    JSON or HTML downloads remain valid; an HTML challenge returned for a ZIP,
    executable, document, or media task cannot be published as a successful file.
    Body snippets are never included in exceptions or logs.
    """

    mime = str(content_type or "").split(";", 1)[0].strip().lower()
    expected_mime = str(getattr(task, "mime_type", "") or "").split(";", 1)[0].strip().lower()
    suffixes = _suffixes(task, final_url, server_filename)
    expects_html = bool(suffixes & _HTML_EXTENSIONS) or expected_mime in {
        "text/html", "application/xhtml+xml",
    }
    expects_json = bool(suffixes & _JSON_EXTENSIONS) or "json" in expected_mime
    expects_binary = bool(suffixes & _STRONG_BINARY_EXTENSIONS) or expected_mime.startswith(("audio/", "video/")) or expected_mime in {
        "application/octet-stream", "application/pdf", "application/zip",
        "application/x-7z-compressed", "application/x-rar-compressed",
    }

    html_response = mime in {"text/html", "application/xhtml+xml"} or _looks_like_html(preview)
    json_response = "json" in mime or _looks_like_json(preview)
    small_document = content_length <= 0 or content_length <= _MAX_ERROR_DOCUMENT_BYTES
    text_error = mime.startswith("text/plain") and small_document and _looks_like_text_error(preview)

    reason = ""
    if html_response and not expects_html:
        reason = "服务器返回了 HTML 登录页、验证页或错误页"
    elif json_response and not expects_json and (expects_binary or small_document):
        reason = "服务器返回了 JSON 错误数据而不是预期文件"
    elif text_error and expects_binary:
        reason = "服务器返回了文本错误信息而不是预期文件"
    if not reason:
        return

    raise DownloadError(DownloadErrorDetails(
        code="HTTP_UNEXPECTED_CONTENT",
        message=reason,
        hint="回到原网页刷新登录或验证状态，再用浏览器插件重新识别；程序已拒绝保存该错误响应。",
        stage=str(getattr(task, "stage", "") or "downloading"),
        url=redact_url(final_url or str(getattr(task, "url", "") or "")),
    ))
