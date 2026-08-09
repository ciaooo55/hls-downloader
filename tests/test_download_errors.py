import asyncio
import errno
import time

import httpx
from curl_cffi.requests.exceptions import ReadTimeout as CurlReadTimeout

from backend.app.downloader.errors import (
    SharedRetryWindow,
    diagnose_download_error,
    retry_delay_seconds,
    should_retry_download_error,
    should_share_retry_window,
)
from backend.app.downloader.hls import HLSDownloader
from backend.app.downloader.parser import UnsupportedPlaylistError
from backend.app.models import Task, TaskStatus
from backend.app.network_proxy import PrivateDestinationError


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

    assert details.code in {"HTTP_403", "HTTP_403_EXPIRED_SIGNATURE"}
    assert details.http_status == 403
    assert details.stage == "downloading_m3u8"
    assert details.url == "https://example.test/video.m3u8"
    assert "浏览器扩展" in details.hint
    assert "Cookie" in details.hint
    assert "403" in details.message


def test_streaming_http_error_diagnostic_does_not_raise_response_not_read():
    request = httpx.Request("GET", "https://example.test/live.m3u8?token=secret")
    response = httpx.Response(403, request=request, stream=httpx.ByteStream(b"expired"))
    try:
        response.raise_for_status()
    except httpx.HTTPStatusError as exc:
        details = diagnose_download_error(exc, stage="parsing", url=str(request.url))
    else:
        raise AssertionError("expected HTTPStatusError")

    assert details.code in {"HTTP_403", "HTTP_403_EXPIRED_SIGNATURE"}
    assert details.http_status == 403
    assert details.url == "https://example.test/live.m3u8"


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

    assert details.code in {"HTTP_403", "HTTP_403_EXPIRED_SIGNATURE"}
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


def test_http_range_and_storage_failures_do_not_masquerade_as_hls_errors():
    range_error = diagnose_download_error(
        RuntimeError("Range 响应缺少有效 Content-Range"),
        stage="downloading",
        url="https://files.test/archive.zip",
    )
    assert range_error.code == "HTTP_RANGE_INVALID"
    assert "拼接错误数据" in range_error.hint

    no_space = OSError(errno.ENOSPC, "No space left on device")
    no_space_details = diagnose_download_error(
        no_space,
        stage="downloading",
        url="https://files.test/archive.zip",
    )
    assert no_space_details.code == "STORAGE_NO_SPACE"

    denied = PermissionError(errno.EACCES, "permission denied")
    denied_details = diagnose_download_error(
        denied,
        stage="downloading",
        url="https://files.test/archive.zip",
    )
    assert denied_details.code == "OUTPUT_PERMISSION_DENIED"


def test_hls_unsupported_messages_do_not_claim_separate_audio_is_unsupported():
    drm = diagnose_download_error(
        UnsupportedPlaylistError("不支持 SAMPLE-AES / DRM 加密"),
        stage="parsing",
    )
    missing_audio = diagnose_download_error(
        UnsupportedPlaylistError("独立 HLS 音轨缺少可下载的 URI"),
        stage="parsing",
    )
    unknown = diagnose_download_error(
        UnsupportedPlaylistError("不支持的 HLS 加密方式: FOO"),
        stage="parsing",
    )

    assert drm.code == "HLS_DRM_UNSUPPORTED"
    assert "官方离线功能" in drm.hint
    assert missing_audio.code == "HLS_AUDIO_TRACK_UNAVAILABLE"
    assert "重新识别" in missing_audio.hint
    assert unknown.code == "HLS_UNSUPPORTED"
    assert "独立音视频 HLS 已支持" in unknown.hint


def test_unrelated_keyword_error_in_merge_is_not_misreported_as_aes_key():
    details = diagnose_download_error(
        TypeError("worker got an unexpected keyword argument 'on_log'"),
        stage="remuxing",
        url="https://example.test/manifest.mpd",
    )

    assert details.code == "FFMPEG_MERGE_FAILED"
    assert "FFmpeg" in details.hint


def test_auth_failure_distinguishes_missing_and_expired_browser_context():
    missing = diagnose_download_error(
        _http_error(403, "https://example.test/file.bin"),
        task_context=Task(id="missing", url="https://example.test/file.bin"),
    )
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

    assert ("缺少网页请求上下文" in missing.hint) or ("网页请求上下文" in missing.hint) or ("Referer" in missing.hint)
    assert ("已过期" in captured.hint) or ("签名" in captured.hint) or ("会话" in captured.hint)
    assert "登录或授权" in unauthorized.hint
    assert should_retry_download_error(_http_error(403)) is False
    assert should_retry_download_error(_http_error(404)) is False
    assert should_retry_download_error(_http_error(429)) is True
    assert should_retry_download_error(_http_error(503)) is True
    assert should_retry_download_error(PrivateDestinationError("private route")) is False


def test_signed_404_recognizes_scoped_browser_request_context():
    url = "https://files.test/backend/content?id=attachment&sig=short-lived"
    details = diagnose_download_error(
        _http_error(404, url),
        stage="probing",
        url=url,
        task_context=Task(
            id="captured-404",
            url=url,
            source_page_url="https://files.test/chat",
            request_contexts={
                "https://files.test": {"request_headers": {"x-session": "captured"}}
            },
        ),
    )

    assert details.code == "HTTP_404"
    assert "已尝试使用原网页" in details.hint
    assert details.url == "https://files.test/backend/content"


def test_proxy_authentication_has_a_specific_recovery_hint():
    details = diagnose_download_error(_http_error(407), stage="downloading_segments")

    assert details.code == "HTTP_407"
    assert "代理服务器" in details.hint
    assert "账号密码" in details.hint
    assert should_retry_download_error(_http_error(407)) is False


def test_retry_delay_honors_valid_retry_after_and_bounds_it():
    request = httpx.Request("GET", "https://example.test/file")
    throttled = httpx.HTTPStatusError(
        "too many", request=request,
        response=httpx.Response(429, headers={"Retry-After": "12"}, request=request),
    )
    excessive = httpx.HTTPStatusError(
        "too many", request=request,
        response=httpx.Response(429, headers={"Retry-After": "999"}, request=request),
    )
    invalid = httpx.HTTPStatusError(
        "too many", request=request,
        response=httpx.Response(429, headers={"Retry-After": "later"}, request=request),
    )

    assert retry_delay_seconds(throttled, 2, maximum=30) == 12
    assert retry_delay_seconds(excessive, 2, maximum=30) == 30
    assert retry_delay_seconds(invalid, 2, maximum=30) == 2


def test_rate_limit_retry_window_is_shared_and_interruptible():
    async def run() -> None:
        window = SharedRetryWindow(poll_interval=0.005)
        await window.extend(0.02)
        started = time.monotonic()
        first = asyncio.create_task(window.wait())
        await asyncio.sleep(0.01)
        remaining, extended = await window.extend(0.05)
        second = asyncio.create_task(window.wait())
        assert extended is True
        assert remaining > 0.04
        assert await first is True
        assert await second is True
        # The second 429 pushed out the first wait rather than each worker
        # continuing independently after its own original backoff.
        assert time.monotonic() - started >= 0.045

        stopped = False
        await window.extend(0.5)

        def stop_check() -> bool:
            return stopped

        waiter = asyncio.create_task(window.wait(stop_check))
        await asyncio.sleep(0.01)
        stopped = True
        assert await waiter is False

    asyncio.run(run())


def test_rate_limit_window_applies_to_429_503_or_retry_after():
    request = httpx.Request("GET", "https://example.test/file")
    limited = httpx.HTTPStatusError(
        "limited", request=request, response=httpx.Response(429, request=request)
    )
    delayed = httpx.HTTPStatusError(
        "busy",
        request=request,
        response=httpx.Response(503, headers={"Retry-After": "1"}, request=request),
    )
    service_unavailable = httpx.HTTPStatusError(
        "busy", request=request, response=httpx.Response(503, request=request)
    )
    transient = httpx.HTTPStatusError(
        "bad gateway", request=request, response=httpx.Response(502, request=request)
    )

    assert should_share_retry_window(limited) is True
    assert should_share_retry_window(delayed) is True
    assert should_share_retry_window(service_unavailable) is True
    assert should_share_retry_window(transient) is False


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

    monkeypatch.setattr(hls_module, "_create_hls_client", lambda *_args: MockClient())
    task = Task(id="failure", url="https://example.test/video.m3u8?token=secret")

    asyncio.run(HLSDownloader(task).run())

    assert task.status is TaskStatus.FAILED
    assert task.error_code in {"HTTP_403", "HTTP_403_EXPIRED_SIGNATURE", "HTTP_403_CLOUDFLARE"}
    assert task.error_stage == "parsing"
    assert task.http_status == 403
    assert task.error_url == "https://example.test/video.m3u8"
    assert "Referer" in task.error_hint
    assert task.error_message.startswith("[HTTP_403")


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

    monkeypatch.setattr(hls_module, "_create_hls_client", lambda *_args: MockClient())
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


def test_failed_hls_keeps_checkpointed_segments_for_retry(tmp_path, monkeypatch):
    from backend.app.downloader import hls as hls_module

    monkeypatch.setattr(hls_module.settings, "download_dir", str(tmp_path))
    monkeypatch.setattr(hls_module.settings, "keep_temp_files", False)
    task = Task(id="resume-failure", url="https://example.test/video.m3u8")
    downloader = HLSDownloader(task)
    segment = {"index": 0, "url": "https://example.test/0.ts", "duration": 4.0}
    downloader._prepare_vod_resume([segment])
    path = downloader._seg_dir() / "000000.seg"
    path.write_bytes(b"verified-segment")
    asyncio.run(downloader._checkpoint_vod_segment(0, path.stat().st_size))

    asyncio.run(downloader._cleanup_failed_temp(downloader._task_dir()))

    assert path.read_bytes() == b"verified-segment"
    assert downloader._vod_state_path().is_file()


def test_http_403_detects_cloudflare_and_signed_url_cases():
    request = httpx.Request("GET", "https://cdn.example.test/video.m3u8?token=abc&expires=1")
    cloudflare = httpx.Response(
        403,
        request=request,
        headers={"server": "cloudflare", "cf-ray": "abc-123"},
        text="Just a moment... cf-browser-verification",
    )
    try:
        cloudflare.raise_for_status()
    except httpx.HTTPStatusError as exc:
        cf_details = diagnose_download_error(exc, stage="downloading_m3u8")
    assert cf_details.code == "HTTP_403_CLOUDFLARE"
    assert "Cloudflare" in cf_details.hint or "人机" in cf_details.hint

    signed = diagnose_download_error(
        _http_error(403, "https://cdn.example.test/seg.ts?X-Amz-Signature=deadbeef&X-Amz-Expires=60"),
        stage="downloading_segments",
        task_context=Task(
            id="signed",
            url="https://cdn.example.test/seg.ts?X-Amz-Signature=deadbeef",
            referer="https://example.test/watch",
            cookie="sid=1",
        ),
    )
    assert signed.code == "HTTP_403_EXPIRED_SIGNATURE"
    assert "签名" in signed.hint

    compact_signed = diagnose_download_error(
        httpx.HTTPStatusError(
            "403",
            request=httpx.Request(
                "GET",
                "https://cdn.test/video.mp4?s=opaque&e=1786000120&_t=1786000030",
            ),
            response=httpx.Response(
                403,
                request=httpx.Request(
                    "GET",
                    "https://cdn.test/video.mp4?s=opaque&e=1786000120&_t=1786000030",
                ),
            ),
        ),
        stage="probing",
        url="https://cdn.test/video.mp4?s=opaque&e=1786000120&_t=1786000030",
    )
    assert compact_signed.code == "HTTP_403_EXPIRED_SIGNATURE"

