import asyncio

import httpx
from curl_cffi.requests.exceptions import ReadTimeout as CurlReadTimeout

from backend.app.downloader.errors import diagnose_download_error, should_retry_download_error
from backend.app.downloader.hls import HLSDownloader
from backend.app.models import Task, TaskStatus


def _http_error(status: int, url: str = "https://example.test/video.m3u8?token=secret"):
    request = httpx.Request("GET", url)
    response = httpx.Response(status, request=request)
    try:
        response.raise_for_status()
    except httpx.HTTPStatusError as exc:
        return exc
    raise AssertionError("expected HTTPStatusError")


def test_http_403_reports_code_stage_redacted_url_and_header_hint():
    details = diagnose_download_error(
        _http_error(403),
        stage="downloading_m3u8",
        url="https://example.test/video.m3u8?token=secret",
    )

    assert details.code == "HTTP_403"
    assert details.http_status == 403
    assert details.stage == "downloading_m3u8"
    assert details.url == "https://example.test/video.m3u8"
    assert "浏览器扩展" in details.hint
    assert "Cookie" in details.hint
    assert "403" in details.message


def test_browser_transport_http_error_keeps_status_and_url():
    class BrowserResponse:
        status_code = 403
        reason = "Forbidden"
        url = "https://cdn.example.test/video.m3u8?token=secret"

    class BrowserHttpError(RuntimeError):
        response = BrowserResponse()

    details = diagnose_download_error(
        BrowserHttpError("browser request failed"),
        stage="parsing",
    )

    assert details.code == "HTTP_403"
    assert details.http_status == 403
    assert details.url == "https://cdn.example.test/video.m3u8"


def test_http_429_and_timeout_have_actionable_hints():
    limited = diagnose_download_error(
        _http_error(429, "https://cdn.example.test/1.ts"),
        stage="downloading_segments",
    )
    timed_out = diagnose_download_error(
        httpx.ReadTimeout("read timed out"),
        stage="downloading_segments",
        url="https://cdn.example.test/2.ts",
    )

    assert limited.code == "HTTP_429"
    assert "降低并发" in limited.hint
    assert timed_out.code == "NETWORK_TIMEOUT"
    assert "网络" in timed_out.hint

    browser_timeout = diagnose_download_error(
        CurlReadTimeout("browser transport timed out"),
        stage="downloading_segments",
        url="https://cdn.example.test/3.ts",
    )
    assert browser_timeout.code == "NETWORK_TIMEOUT"


def test_auth_failure_distinguishes_missing_and_expired_browser_context():
    missing = diagnose_download_error(_http_error(403), task_context=Task(id="missing", url="https://example.test/file"))
    captured = diagnose_download_error(
        _http_error(403),
        task_context=Task(
            id="captured",
            url="https://example.test/file",
            request_headers={"authorization": "Bearer old"},
            referer="https://example.test/watch",
        ),
    )
    unauthorized = diagnose_download_error(_http_error(401), task_context=Task(id="login", url="https://example.test/file"))

    assert "缺少网页请求上下文" in missing.hint
    assert "已过期" in captured.hint
    assert "登录或授权" in unauthorized.hint
    assert should_retry_download_error(_http_error(403)) is False
    assert should_retry_download_error(_http_error(404)) is False
    assert should_retry_download_error(_http_error(429)) is True
    assert should_retry_download_error(_http_error(503)) is True


def test_proxy_authentication_has_a_specific_recovery_hint():
    details = diagnose_download_error(_http_error(407), stage="downloading_segments")

    assert details.code == "HTTP_407"
    assert "代理服务器" in details.hint
    assert "账号密码" in details.hint
    assert should_retry_download_error(_http_error(407)) is False


def test_range_and_merge_failures_get_stable_codes():
    ranged = diagnose_download_error(
        RuntimeError("Content-Range 不匹配，期望 2-5，实际 0-3"),
        stage="downloading_segments",
        url="https://cdn.example.test/file.bin",
    )
    merged = diagnose_download_error(
        RuntimeError("ffmpeg exited with code 1"),
        stage="merging",
    )

    assert ranged.code == "HLS_RANGE_INVALID"
    assert "Range" in ranged.hint
    assert merged.code == "FFMPEG_MERGE_FAILED"
    assert "FFmpeg" in merged.hint


def test_output_verification_failure_uses_ffmpeg_code():
    details = diagnose_download_error(
        RuntimeError("输出时长异常，期望约 10.0s，实际 2.0s"),
        stage="verifying",
    )

    assert details.code == "FFMPEG_MERGE_FAILED"


def test_playlist_http_failure_is_persisted_on_task(tmp_path, monkeypatch):
    from backend.app.downloader import hls as hls_module

    monkeypatch.setattr(hls_module.settings, "download_dir", str(tmp_path))

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(403, request=request)

    class MockClient(httpx.AsyncClient):
        def __init__(self, *args, **kwargs):
            kwargs["transport"] = httpx.MockTransport(handler)
            super().__init__(*args, **kwargs)

    monkeypatch.setattr(hls_module, "_create_hls_client", lambda _concurrency: MockClient())
    task = Task(id="failure", url="https://example.test/video.m3u8?token=secret")

    asyncio.run(HLSDownloader(task).run())

    assert task.status is TaskStatus.FAILED
    assert task.error_code == "HTTP_403"
    assert task.error_stage == "parsing"
    assert task.http_status == 403
    assert task.error_url == "https://example.test/video.m3u8"
    assert "Referer" in task.error_hint
    assert task.error_message.startswith("[HTTP_403]")


def test_failed_download_keeps_log_but_removes_large_temp_data(tmp_path, monkeypatch):
    from backend.app.downloader import hls as hls_module

    monkeypatch.setattr(hls_module.settings, "download_dir", str(tmp_path))
    monkeypatch.setattr(hls_module.settings, "keep_temp_files", False)

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(403, request=request)

    class MockClient(httpx.AsyncClient):
        def __init__(self, *args, **kwargs):
            kwargs["transport"] = httpx.MockTransport(handler)
            super().__init__(*args, **kwargs)

    monkeypatch.setattr(hls_module, "_create_hls_client", lambda _concurrency: MockClient())
    task = Task(id="keep-log", url="https://example.test/video.m3u8")
    task_dir = tmp_path / ".tasks" / task.id
    segments = task_dir / "segments"
    segments.mkdir(parents=True)
    (segments / "partial.tmp").write_bytes(b"x" * 1024)

    def write_log(_task_id: str, message: str) -> None:
        task_dir.mkdir(parents=True, exist_ok=True)
        with (task_dir / "download.log").open("a", encoding="utf-8") as handle:
            handle.write(message + "\n")

    asyncio.run(HLSDownloader(task, on_log=write_log).run())

    assert task.status is TaskStatus.FAILED
    assert (task_dir / "download.log").is_file()
    assert not segments.exists()
