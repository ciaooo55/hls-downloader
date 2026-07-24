import hashlib
import io
import json
import time
import urllib.error
from dataclasses import replace
from email.message import Message
from types import SimpleNamespace

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from backend.app import updater
from backend.app import api as api_module
from backend.app.updater import UpdateError, UpdateInfo


class FakeResponse(io.BytesIO):
    def __init__(self, data: bytes, url: str = "") -> None:
        super().__init__(data)
        self.url = url

    def geturl(self):
        return self.url

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.close()


def _info(data: bytes, **changes) -> UpdateInfo:
    base = UpdateInfo(
        current_version="1.0.0",
        latest_version="9.0.0",
        available=True,
        can_auto_install=True,
        release_url="https://github.com/ciaooo55/hls-downloader/releases/tag/v9.0.0",
        download_url="https://github.com/ciaooo55/hls-downloader/releases/download/v9.0.0/HLSDownloader-Windows-x64-Setup.exe",
        size=len(data),
        digest=hashlib.sha256(data).hexdigest(),
        notes="release notes",
    )
    return replace(base, **changes)


def test_semantic_version_comparison_handles_prefixes_and_padding():
    assert updater.is_newer_version("v1.2.0", "1.1.9") is True
    assert updater.is_newer_version("1.2", "1.2.0") is False
    assert updater.is_newer_version("1.1.9", "1.2.0") is False
    with pytest.raises(UpdateError):
        updater.is_newer_version("latest", "1.0.0")


def test_update_check_selects_exact_windows_asset_and_digest(monkeypatch):
    data = b"MZsetup"
    payload = {
        "tag_name": "v9.0.0",
        "html_url": "https://github.com/ciaooo55/hls-downloader/releases/tag/v9.0.0",
        "body": "fixed things",
        "assets": [
            {"name": "other.zip", "size": 1, "digest": "sha256:" + "0" * 64},
            {
                "name": updater.SETUP_ASSET_NAME,
                "size": len(data),
                "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
                "browser_download_url": "https://github.com/ciaooo55/hls-downloader/releases/download/v9.0.0/HLSDownloader-Windows-x64-Setup.exe",
            },
        ],
    }
    monkeypatch.setattr(
        updater,
        "RUNTIME_PATHS",
        SimpleNamespace(mode="installed", data_root=None),
    )

    info = updater.check_for_update(
        opener=lambda request, timeout: FakeResponse(json.dumps(payload).encode())
    )

    assert info.available is True
    assert info.latest_version == "9.0.0"
    assert info.can_auto_install is True
    assert info.digest == hashlib.sha256(data).hexdigest()


def test_update_check_rejects_untrusted_download_host():
    payload = {
        "tag_name": "v9.0.0",
        "assets": [{
            "name": updater.SETUP_ASSET_NAME,
            "size": 10,
            "digest": "sha256:" + "0" * 64,
            "browser_download_url": "https://example.test/fake.exe",
        }],
    }
    with pytest.raises(UpdateError, match="不可信"):
        updater.check_for_update(
            opener=lambda request, timeout: FakeResponse(json.dumps(payload).encode())
        )


def test_update_check_falls_back_to_release_checksums_when_api_is_limited(monkeypatch):
    digest = "a" * 64
    calls: list[str] = []

    def opener(request, timeout):
        calls.append(request.full_url)
        if request.full_url == updater.LATEST_RELEASE_API:
            raise OSError("rate limit")
        if request.full_url == updater.LATEST_RELEASE_PAGE:
            return FakeResponse(
                b"",
                "https://github.com/ciaooo55/hls-downloader/releases/tag/v9.0.0",
            )
        return FakeResponse(f"{digest}  {updater.SETUP_ASSET_NAME}\n".encode())

    monkeypatch.setattr(
        updater,
        "RUNTIME_PATHS",
        SimpleNamespace(mode="installed", data_root=None),
    )
    info = updater.check_for_update(opener=opener)

    assert info.latest_version == "9.0.0"
    assert info.digest == digest
    assert info.size == 0
    assert info.can_auto_install is True
    assert calls[-1].endswith("/v9.0.0/SHA256SUMS.txt")


def test_rate_limited_check_returns_a_safe_actionable_error(monkeypatch):
    headers = Message()
    headers["X-RateLimit-Remaining"] = "0"
    headers["X-RateLimit-Reset"] = str(int(time.time()) + 120)

    def opener(request, timeout):
        if request.full_url == updater.LATEST_RELEASE_API:
            raise urllib.error.HTTPError(
                request.full_url,
                403,
                "Forbidden",
                headers,
                io.BytesIO(b'{"message":"API rate limit exceeded"}'),
            )
        raise urllib.error.URLError("[SSL: UNEXPECTED_EOF_WHILE_READING] eof")

    with pytest.raises(updater.UpdateCheckError) as raised:
        updater.check_for_update(opener=opener)

    error = raised.value
    assert error.code == "GITHUB_RATE_LIMITED"
    assert error.retry_after_seconds is not None
    assert "GitHub" in str(error)
    assert "SSL" not in str(error)
    assert "urlopen" not in str(error)


def test_release_checksum_tls_error_is_not_exposed_to_clients():
    def opener(request, timeout):
        raise urllib.error.URLError("<urlopen error [SSL: UNEXPECTED_EOF_WHILE_READING]>")

    with pytest.raises(updater.UpdateCheckError) as raised:
        updater.check_for_update(opener=opener)

    assert raised.value.code == "NETWORK_ERROR"
    assert "SSL" not in str(raised.value)
    assert "urlopen" not in str(raised.value)


def test_installer_download_is_atomic_and_sha256_verified(tmp_path):
    data = b"MZ" + b"installer" * 100
    info = _info(data)

    result = updater.download_installer(
        info,
        opener=lambda request, timeout: FakeResponse(data),
        destination_root=tmp_path,
    )

    assert result.read_bytes() == data
    assert not list(tmp_path.glob("*.part"))


def test_installer_download_uses_configured_download_directory(monkeypatch, tmp_path):
    data = b"MZ" + b"installer" * 10
    info = _info(data)
    downloads = tmp_path / "Downloads" / "HLS Downloader"
    monkeypatch.setattr(updater, "get_update_directory", lambda: downloads)

    result = updater.download_installer(
        info,
        opener=lambda request, timeout: FakeResponse(data),
    )

    assert result.parent == downloads
    assert result.name == "HLSDownloader-Update-9.0.0.exe"


def test_installer_download_removes_partial_file_on_bad_hash(tmp_path):
    data = b"MZbroken"
    info = _info(data, digest="0" * 64)

    with pytest.raises(UpdateError, match="SHA-256"):
        updater.download_installer(
            info,
            opener=lambda request, timeout: FakeResponse(data),
            destination_root=tmp_path,
        )

    assert not list(tmp_path.iterdir())


def test_update_cache_cleanup_removes_only_update_installers(monkeypatch, tmp_path):
    downloads = tmp_path / "downloads"
    legacy = tmp_path / "data" / "updates"
    downloads.mkdir()
    legacy.mkdir(parents=True)
    stale = downloads / "HLSDownloader-Update-8.0.0.exe"
    partial = legacy / "HLSDownloader-Update-8.0.0.exe.part"
    unrelated = downloads / "keep-me.exe"
    stale.write_bytes(b"old")
    partial.write_bytes(b"partial")
    unrelated.write_bytes(b"keep")
    monkeypatch.setattr(updater, "get_update_directory", lambda: downloads)
    monkeypatch.setattr(
        updater,
        "RUNTIME_PATHS",
        SimpleNamespace(mode="installed", data_root=tmp_path / "data"),
    )

    updater.cleanup_update_cache()

    assert not stale.exists()
    assert not partial.exists()
    assert unrelated.exists()


def test_update_service_never_launches_installer_twice(monkeypatch, tmp_path):
    data = b"MZsetup"
    info = _info(data)
    service = updater.UpdateService()
    launched: list[list[str]] = []
    installer = tmp_path / "setup.exe"
    installer.write_bytes(data)
    monkeypatch.setattr(service, "check", lambda force: info)
    monkeypatch.setattr(updater, "download_installer", lambda _info: installer)

    assert service.download_and_launch(process_starter=lambda args: launched.append(args)) == info
    with pytest.raises(UpdateError, match="已经启动"):
        service.download_and_launch(process_starter=lambda args: launched.append(args))

    assert launched == [[str(installer), "/DELETESELF=1"]]


def test_update_service_installs_from_recent_verified_cache_without_rechecking(monkeypatch, tmp_path):
    data = b"MZsetup"
    info = _info(data)
    service = updater.UpdateService()
    service._cache = (time.monotonic(), info)
    installer = tmp_path / "setup.exe"
    installer.write_bytes(data)
    monkeypatch.setattr(service, "check", lambda force: pytest.fail("must use trusted cache"))
    monkeypatch.setattr(updater, "download_installer", lambda cached: installer)

    result = service.download_and_launch(process_starter=lambda _args: None)

    assert result == info


def test_update_api_requires_token_and_returns_release_state(monkeypatch):
    info = _info(b"MZsetup")
    monkeypatch.setattr(api_module.update_service, "check", lambda force=False: info)
    test_app = FastAPI()
    test_app.include_router(api_module.router)

    with TestClient(test_app) as client:
        unauthorized = client.get("/api/update/check")
        response = client.get("/api/update/check?force=true", headers={"X-Token": "55555"})

    assert unauthorized.status_code == 401
    assert response.status_code == 200
    assert response.json()["latest_version"] == "9.0.0"
    assert response.json()["available"] is True


def test_update_api_rejects_duplicate_installer_launch(monkeypatch):
    def duplicate():
        raise UpdateError("更新安装程序已经启动")

    monkeypatch.setattr(api_module.update_service, "download_and_launch", duplicate)
    test_app = FastAPI()
    test_app.include_router(api_module.router)

    with TestClient(test_app) as client:
        response = client.post("/api/update/install", headers={"X-Token": "55555"})

    assert response.status_code == 409
    assert "已经启动" in response.json()["detail"]
