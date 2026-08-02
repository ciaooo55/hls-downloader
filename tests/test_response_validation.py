import pytest

from backend.app.downloader.errors import DownloadError
from backend.app.downloader.response_validation import validate_download_response
from backend.app.models import Task, TaskType


def _task(url: str, filename: str = "") -> Task:
    return Task(id="validate", url=url, filename=filename, task_type=TaskType.HTTP)


def test_rejects_html_challenge_returned_for_binary_download():
    task = _task("https://cdn.test/archive.zip", "archive.zip")

    with pytest.raises(DownloadError) as raised:
        validate_download_response(
            task,
            content_type="text/html; charset=utf-8",
            content_length=512,
            preview=b"<!doctype html><html><title>Login</title></html>",
            final_url="https://login.test/challenge?token=private",
        )

    assert raised.value.details.code == "HTTP_UNEXPECTED_CONTENT"
    assert raised.value.details.url == "https://login.test/challenge"
    assert "token" not in str(raised.value)


def test_rejects_json_error_returned_for_media_download():
    task = _task("https://cdn.test/movie.mp4", "movie.mp4")

    with pytest.raises(DownloadError) as raised:
        validate_download_response(
            task,
            content_type="application/json",
            content_length=48,
            preview=b'{"error":"signed URL expired"}',
        )

    assert raised.value.details.code == "HTTP_UNEXPECTED_CONTENT"


def test_allows_explicit_json_and_html_downloads():
    validate_download_response(
        _task("https://api.test/export", "report.json"),
        content_type="application/json",
        content_length=128,
        preview=b'{"items":[1,2,3]}',
    )
    validate_download_response(
        _task("https://site.test/page", "saved.html"),
        content_type="text/html",
        content_length=128,
        preview=b"<!doctype html><html></html>",
    )


def test_allows_normal_binary_payload():
    validate_download_response(
        _task("https://cdn.test/archive.zip", "archive.zip"),
        content_type="application/zip",
        content_length=2048,
        preview=b"PK\x03\x04" + b"x" * 32,
    )
