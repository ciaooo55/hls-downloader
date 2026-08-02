import hashlib
import io
import json
import os
from pathlib import Path
import time
import urllib.error
import asyncio
import zipfile
from dataclasses import replace
from email.message import Message
from types import SimpleNamespace

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from backend.app import updater
from backend.app import api as api_module
from backend.app.config import settings
from backend.app.models import Task, TaskStatus
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
        download_url="https://github.com/ciaooo55/hls-downloader/releases/download/v9.0.0/HLSDownloader-v9.0.0-Windows-x64-Setup.exe",
        size=len(data),
        digest=hashlib.sha256(data).hexdigest(),
        notes="release notes",
    )
    return replace(base, **changes)


def _portable_bytes() -> bytes:
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as package:
        package.writestr("HLSDownloader.exe", b"MZdesktop")
        package.writestr("HLSDownloaderCore.exe", b"MZcore")
        package.writestr("portable", b"")
        package.writestr("scripts/upgrade-portable.ps1", b"Write-Host upgrade")
        package.writestr("scripts/shutdown-running.ps1", b"exit 0")
    return output.getvalue()


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
                "name": updater.setup_asset_name("9.0.0"),
                "size": len(data),
                "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
                "browser_download_url": "https://github.com/ciaooo55/hls-downloader/releases/download/v9.0.0/HLSDownloader-v9.0.0-Windows-x64-Setup.exe",
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
            "name": updater.setup_asset_name("9.0.0"),
            "size": 10,
            "digest": "sha256:" + "0" * 64,
            "browser_download_url": "https://example.test/fake.exe",
        }],
    }
    with pytest.raises(UpdateError, match="不可信"):
        updater.check_for_update(
            opener=lambda request, timeout: FakeResponse(json.dumps(payload).encode())
        )


def test_portable_update_check_selects_portable_asset(monkeypatch, tmp_path):
    data = _portable_bytes()
    root = tmp_path / "portable"
    (root / "scripts").mkdir(parents=True)
    (root / "portable").write_bytes(b"")
    (root / "scripts" / "upgrade-portable.ps1").write_text("# upgrade", encoding="utf-8")
    payload = {
        "tag_name": "v9.0.0",
        "assets": [{
            "name": updater.portable_asset_name("9.0.0"),
            "size": len(data),
            "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
            "browser_download_url": (
                "https://github.com/ciaooo55/hls-downloader/releases/download/"
                "v9.0.0/HLSDownloader-v9.0.0-Windows-x64-Portable.zip"
            ),
        }],
    }
    monkeypatch.setattr(
        updater,
        "RUNTIME_PATHS",
        SimpleNamespace(mode="portable", project_root=root, data_root=root),
    )

    info = updater.check_for_update(
        opener=lambda request, timeout: FakeResponse(json.dumps(payload).encode())
    )

    assert info.asset_kind == "portable"
    assert info.can_auto_install is True
    assert info.download_url.endswith("-Portable.zip")


def test_portable_archive_validation_and_safe_extraction(tmp_path):
    archive = tmp_path / "portable.zip"
    archive.write_bytes(_portable_bytes())

    updater.validate_portable_archive(archive)
    extracted = updater.extract_portable_update(archive, "9.0.0")
    try:
        assert (extracted / "HLSDownloader.exe").is_file()
        assert (extracted / "scripts" / "upgrade-portable.ps1").is_file()
    finally:
        import shutil

        shutil.rmtree(extracted, ignore_errors=True)


def test_portable_archive_rejects_path_traversal(tmp_path):
    archive = tmp_path / "unsafe.zip"
    with zipfile.ZipFile(archive, "w") as package:
        package.writestr("../outside.txt", b"bad")
    with pytest.raises(UpdateError, match="不安全"):
        updater.validate_portable_archive(archive)


def test_update_check_does_not_depend_on_an_unpublished_checksum_asset():
    def opener(request, timeout):
        raise OSError("offline")

    with pytest.raises(updater.UpdateCheckError, match="网络"):
        updater.check_for_update(opener=opener)


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


def test_rate_limited_check_falls_back_to_latest_release_redirect(monkeypatch):
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
        assert request.full_url == updater.LATEST_RELEASE_PAGE
        return FakeResponse(
            b"<html></html>",
            "https://github.com/ciaooo55/hls-downloader/releases/tag/v9.0.0",
        )

    monkeypatch.setattr(updater, "APP_VERSION", "1.0.0")
    monkeypatch.setattr(updater, "RUNTIME_PATHS", SimpleNamespace(mode="installed", data_root=None))

    info = updater.check_for_update(opener=opener)

    assert info.available is True
    assert info.latest_version == "9.0.0"
    assert info.can_auto_install is False
    assert info.release_url.endswith("/tag/v9.0.0")
    assert not info.digest


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
    old = time.time() - updater.UPDATE_CACHE_MAX_AGE_SECONDS - 60
    os.utime(stale, (old, old))
    os.utime(partial, (old, old))
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


def test_update_cache_cleanup_preserves_fresh_resumable_downloads(monkeypatch, tmp_path):
    downloads = tmp_path / "downloads"
    updates = tmp_path / "data" / "updates"
    downloads.mkdir()
    updates.mkdir(parents=True)
    installer = downloads / "HLSDownloader-Update-9.0.0.exe"
    partial = updates / "HLSDownloader-Update-9.0.0.exe.part"
    installer.write_bytes(b"MZfresh")
    partial.write_bytes(b"resumable")
    monkeypatch.setattr(updater, "get_update_directory", lambda: downloads)
    monkeypatch.setattr(updater, "RUNTIME_PATHS", SimpleNamespace(data_root=tmp_path / "data"))

    updater.cleanup_update_cache(now=time.time())

    assert installer.read_bytes() == b"MZfresh"
    assert partial.read_bytes() == b"resumable"


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
        response = client.get("/api/update/check?force=true", headers={"X-Token": settings.token})

    assert unauthorized.status_code == 401
    assert response.status_code == 200
    assert response.json()["latest_version"] == "9.0.0"
    assert response.json()["available"] is True


def test_update_api_rejects_duplicate_installer_launch(monkeypatch):
    def duplicate():
        raise UpdateError("更新安装程序已经启动")

    monkeypatch.setattr(api_module.update_service, "prepare_managed_download", duplicate)
    test_app = FastAPI()
    test_app.include_router(api_module.router)

    with TestClient(test_app) as client:
        response = client.post("/api/update/install", headers={"X-Token": settings.token})

    assert response.status_code == 409
    assert "已经启动" in response.json()["detail"]


def test_managed_update_is_a_resumable_download_task(monkeypatch, tmp_path):
    info = _info(b"MZsetup")

    class FakeManager:
        def __init__(self):
            self.tasks = {}
            self.started = []

        async def create_task(self, **kwargs):
            task = Task(
                id="managed-update",
                url=kwargs["url"],
                task_type=kwargs["task_type"],
                filename=kwargs["filename"],
                expected_checksum=kwargs["checksum"],
            )
            self.tasks[task.id] = task
            return task

        async def save_task(self, _task):
            return None

        async def start_task(self, task_id):
            self.started.append(task_id)

        async def resume_task(self, task_id):
            self.started.append(task_id)

    monkeypatch.setattr(updater, "get_update_directory", lambda: tmp_path)
    manager = FakeManager()
    task = __import__("asyncio").run(updater.queue_update_download(info, manager))

    assert task.engine_state["is_update"] is True
    assert task.engine_state["update_identity"] == f"installer:{info.latest_version}:{info.digest}"
    assert manager.started == [task.id]


def test_failed_managed_update_task_is_retried(monkeypatch, tmp_path):
    info = _info(b"MZsetup")

    class FakeManager:
        def __init__(self):
            task = Task(id="failed-update", url=info.download_url)
            task.status = TaskStatus.FAILED
            task.engine_state["update_identity"] = f"installer:{info.latest_version}:{info.digest}"
            self.tasks = {task.id: task}
            self.retried = []

        async def retry_task(self, task_id):
            self.retried.append(task_id)

        async def save_task(self, _task):
            return None

    manager = FakeManager()
    task = __import__("asyncio").run(updater.queue_update_download(info, manager))

    assert task.id == "failed-update"
    assert manager.retried == [task.id]


def test_missing_completed_update_file_is_downloaded_again(tmp_path):
    info = _info(b"MZsetup")

    class FakeManager:
        def __init__(self):
            task = Task(id="missing-update", url=info.download_url)
            task.status = TaskStatus.DONE
            task.output_path = str(tmp_path / "removed-setup.exe")
            task.engine_state["update_identity"] = f"installer:{info.latest_version}:{info.digest}"
            self.tasks = {task.id: task}
            self.retried = []

        async def save_task(self, _task):
            return None

        async def retry_task(self, task_id):
            self.retried.append(task_id)

    manager = FakeManager()
    task = asyncio.run(updater.queue_update_download(info, manager))

    assert task.status is TaskStatus.FAILED
    assert task.error_code == "UPDATE_INSTALLER_MISSING"
    assert manager.retried == [task.id]


def test_reused_managed_update_restores_trusted_launch_metadata(monkeypatch, tmp_path):
    data = b"MZsetup"
    info = _info(data)

    class FakeManager:
        def __init__(self):
            task = Task(id="restored-update", url=info.download_url)
            task.status = TaskStatus.DONE
            task.output_path = str(tmp_path / "setup.exe")
            task.engine_state["update_identity"] = f"installer:{info.latest_version}:{info.digest}"
            self.tasks = {task.id: task}
            self.saved = []

        async def save_task(self, task):
            self.saved.append(task.id)

    Path(tmp_path / "setup.exe").write_bytes(data)
    manager = FakeManager()

    task = asyncio.run(updater.queue_update_download(info, manager))

    assert task.expected_checksum == f"sha256:{info.digest}"
    assert task.engine_state["update_expected_size"] == len(data)
    assert manager.saved == [task.id]


def test_managed_update_launch_requires_hash_and_prepares_before_start(monkeypatch, tmp_path):
    data = b"MZsetup"
    info = _info(data)
    installer = tmp_path / "setup.exe"
    installer.write_bytes(data)
    task = Task(id="launch-update", url=info.download_url)
    task.status = TaskStatus.DONE
    task.output_path = str(installer)
    task.expected_checksum = f"sha256:{info.digest}"
    task.engine_state["update_expected_size"] = len(data)
    order = []

    class FakeManager:
        tasks = {task.id: task}

        async def prepare_for_update_restart(self):
            order.append("prepare")

        async def save_task(self, _task):
            order.append("save")

    class FakeTimer:
        daemon = False

        def __init__(self, _delay, _callback):
            order.append("timer-created")

        def start(self):
            order.append("timer-started")

    monkeypatch.setattr(api_module, "manager", FakeManager())
    monkeypatch.setattr(api_module.subprocess, "Popen", lambda _args: order.append("launch"))
    monkeypatch.setattr(api_module.threading, "Timer", FakeTimer)
    monkeypatch.setattr(api_module.update_service, "_install_started", False)

    asyncio.run(api_module._launch_managed_update(task.id))

    assert order[:2] == ["prepare", "launch"]
    assert api_module.update_service._install_started is True


def test_managed_update_launch_rejects_missing_checksum(monkeypatch, tmp_path):
    installer = tmp_path / "setup.exe"
    installer.write_bytes(b"MZsetup")
    task = Task(id="unsafe-update", url="https://github.com/example")
    task.status = TaskStatus.DONE
    task.output_path = str(installer)
    task.engine_state["update_expected_size"] = installer.stat().st_size
    saved = []

    class FakeManager:
        tasks = {task.id: task}

        async def prepare_for_update_restart(self):
            pytest.fail("unsafe installer must not reach update preparation")

        async def save_task(self, saved_task):
            saved.append(saved_task.id)

    monkeypatch.setattr(api_module, "manager", FakeManager())
    monkeypatch.setattr(api_module.subprocess, "Popen", lambda _args: pytest.fail("must not launch"))

    asyncio.run(api_module._launch_managed_update(task.id))

    assert task.status is TaskStatus.FAILED
    assert task.error_code == "UPDATE_INSTALLER_INVALID"
    assert saved == [task.id]
    assert not installer.exists()


def test_managed_portable_update_launches_transactional_upgrade(monkeypatch, tmp_path):
    data = _portable_bytes()
    archive = tmp_path / "portable.zip"
    archive.write_bytes(data)
    current = tmp_path / "current"
    current.mkdir()
    stage = tmp_path / "extracted"
    (stage / "scripts").mkdir(parents=True)
    upgrade_script = stage / "scripts" / "upgrade-portable.ps1"
    upgrade_script.write_text("# upgrade", encoding="utf-8")
    task = Task(id="portable-update", url="https://github.com/example")
    task.status = TaskStatus.DONE
    task.output_path = str(archive)
    task.expected_checksum = f"sha256:{hashlib.sha256(data).hexdigest()}"
    task.engine_state.update({
        "update_expected_size": len(data),
        "update_asset_kind": "portable",
        "update_version": "9.0.0",
    })
    order: list[object] = []

    class FakeManager:
        tasks = {task.id: task}

        async def prepare_for_update_restart(self):
            order.append("prepare")

        async def save_task(self, _task):
            order.append("save")

    class FakeTimer:
        daemon = False

        def __init__(self, _delay, _callback):
            order.append("timer-created")

        def start(self):
            order.append("timer-started")

    monkeypatch.setattr(api_module, "manager", FakeManager())
    monkeypatch.setattr(
        api_module,
        "RUNTIME_PATHS",
        SimpleNamespace(mode="portable", project_root=current),
    )
    monkeypatch.setattr(api_module, "extract_portable_update", lambda *_args: stage)
    monkeypatch.setattr(api_module.subprocess, "Popen", lambda args: order.append(args))
    monkeypatch.setattr(api_module.threading, "Timer", FakeTimer)
    monkeypatch.setattr(api_module.update_service, "_install_started", False)

    asyncio.run(api_module._launch_managed_update(task.id))

    assert order[0] == "prepare"
    launch = order[1]
    assert isinstance(launch, list)
    assert launch[0] == "powershell.exe"
    assert str(upgrade_script) in launch
    assert str(current) in launch
    assert "-DeleteSourceAfterUpgrade" in launch
    assert api_module.update_service._install_started is True
