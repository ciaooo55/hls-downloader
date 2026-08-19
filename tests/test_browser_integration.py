import time
import base64

from backend.app.browser_handoff import (
    MAX_BROWSER_CLIENTS,
    MAX_BROWSER_HANDOFFS,
    BROWSER_EXTENSION_RELEASE_URL,
    BrowserHandoffService,
    MIN_BROWSER_EXTENSION_VERSION,
    RECOMMENDED_BROWSER_EXTENSION_VERSION,
)
from backend.app.credentials import PREFIX, protect_secret, unprotect_secret
from backend.app.version import APP_VERSION
from backend import native_host


def test_versioned_native_host_resolves_the_install_root(tmp_path):
    install_root = tmp_path / "HLS Downloader"
    host = (
        install_root
        / "native-host"
        / "versions"
        / "HLSDownloaderNativeHost-1.10.0.exe"
    )
    host.parent.mkdir(parents=True)
    host.write_bytes(b"host")
    (install_root / "HLSDownloader.exe").write_bytes(b"desktop")

    assert native_host._frozen_install_root(host) == install_root


def test_legacy_native_host_keeps_its_existing_root(tmp_path):
    install_root = tmp_path / "HLS Downloader"
    host = install_root / "HLSDownloaderNativeHost.exe"
    host.parent.mkdir(parents=True)
    host.write_bytes(b"host")
    (install_root / "portable").write_text("", encoding="ascii")

    assert native_host._frozen_install_root(host) == install_root


def test_browser_handoff_confirmation_and_expiry():
    service = BrowserHandoffService(ttl=1.0)
    item = service.create({
        "url": "https://cdn.test/file.zip",
        "cookie": "session=secret",
        "request_contexts": {
            "https://segments.test": {
                "request_headers": {"authorization": "Bearer scoped"},
                "cookie": "scoped=secret",
            }
        },
        "request_method": "POST",
        "request_body": base64.b64encode(b'{"download":"protected"}').decode("ascii"),
        "request_headers": {
            "authorization": "Bearer private",
            "content-type": "application/json",
        },
        "size": 42,
    })
    assert item.status == "pending"
    assert "cookie" not in item.public()
    assert "request_headers" not in item.public()
    assert "request_contexts" not in item.public()
    assert "request_body" not in item.public()
    assert item.request_headers == {"authorization": "Bearer private", "content-type": "application/json"}
    assert item.request_contexts["https://segments.test"]["cookie"] == "scoped=secret"
    assert item.detail()["effective_context"]["target_origin"] == "https://cdn.test"
    assert item.detail()["effective_context"]["cookie"] == "session=secret"
    assert item.request_method == "POST"
    assert item.request_body
    assert service.pending()[0]["id"] == item.id
    assert service.reject(item.id).status == "rejected"

    expired = service.create({"url": "https://cdn.test/old.zip"})
    expired.created_at = time.time() - 2.0
    assert service.get(expired.id).status == "expired"


def test_browser_handoff_keeps_bounded_recognition_contract_without_secrets():
    service = BrowserHandoffService()
    item = service.create({
        "url": "https://cdn.test/live.m3u8",
        "evidence": ["current_src", "mse_source_buffer", "x" * 5000],
        "owner": "mse-source-buffer",
        "confidence": 1.7,
        "replay_context": {
            "method": "GET",
            "request_id": "req-1",
            "cookie": "must-not-be-a-contract-field",
        },
        "cookie": "session=secret",
    })

    assert item.evidence == ["current_src", "mse_source_buffer", "x" * 64]
    assert item.owner == "mse-source-buffer"
    assert item.confidence == 1.0
    assert item.replay_context == {
        "method": "GET",
        "request_id": "req-1",
    }
    public = item.public()
    assert public["evidence"] == item.evidence
    assert public["owner"] == item.owner
    assert "cookie" not in public


def test_browser_handoff_bounds_replay_metadata_and_rejects_non_finite_confidence():
    service = BrowserHandoffService()
    item = service.create({
        "url": "https://cdn.test/file.bin",
        "confidence": float("nan"),
        "replay_context": {f"field_{index}": str(index) for index in range(20)},
    })

    assert item.confidence == 0.0
    assert len(item.replay_context) == 12
    assert item.replay_context["field_0"] == "0"
    assert item.replay_context["field_11"] == "11"


def test_browser_handoff_transport_retry_is_idempotent():
    service = BrowserHandoffService()
    payload = {
        "url": "https://cdn.test/live.m3u8?token=fresh",
        "extension_client_id": "edge-install",
        "client_request_id": "offer-7f6ccf5d",
    }

    first = service.create(payload)
    retried = service.create(payload)

    assert retried is first
    assert retried.id == first.id
    assert len(service.pending()) == 1


def test_browser_handoff_allows_a_new_offer_after_terminal_resolution():
    service = BrowserHandoffService()
    payload = {
        "url": "https://cdn.test/live.m3u8?token=fresh",
        "extension_client_id": "edge-install",
        "client_request_id": "resource:7:0123456789abcdef0123456789abcdef",
    }

    first = service.create(payload)
    service.reject(first.id)
    second = service.create(payload)

    assert second.id != first.id
    assert second.status == "pending"


def test_browser_handoff_replaces_generic_manifest_name_with_page_title():
    service = BrowserHandoffService()
    item = service.create({
        "url": "https://cdn.test/video_1080p.m3u8?token=1",
        "filename": "video_1080p.m3u8",
        "title": "第十二集：重新出发",
        "source_page_url": "https://site.test/watch/episode-12",
        "mime_type": "application/vnd.apple.mpegurl",
    })

    assert item.filename == "第十二集：重新出发"


def test_browser_status_explains_when_extension_has_never_connected():
    service = BrowserHandoffService()

    assert service.status() == {
        "detected": False,
        "seen_before": False,
        "version": "",
        "state": "not_detected",
        "message": "未检测到浏览器扩展；浏览器下载不会被接管",
        "desktop_version": APP_VERSION,
        "recommended_version": RECOMMENDED_BROWSER_EXTENSION_VERSION,
        "minimum_version": MIN_BROWSER_EXTENSION_VERSION,
        "release_url": BROWSER_EXTENSION_RELEASE_URL,
        "needs_upgrade": False,
        "clients": [],
        "active_versions": [],
        "client_count": 0,
    }


def test_browser_status_marks_outdated_extension_without_blocking_connection():
    service = BrowserHandoffService()
    service.record_ping("2.0.9")

    status = service.status()

    assert status["detected"] is True
    assert status["version"] == "2.0.9"
    assert status["recommended_version"] == RECOMMENDED_BROWSER_EXTENSION_VERSION
    assert status["minimum_version"] == MIN_BROWSER_EXTENSION_VERSION
    assert status["needs_upgrade"] is True
    assert "建议升级" in status["message"]


def test_browser_status_recommends_upgrade_for_previous_compatible_extension():
    service = BrowserHandoffService()
    service.record_ping(MIN_BROWSER_EXTENSION_VERSION)

    status = service.status()

    assert status["detected"] is True
    assert status["version"] == MIN_BROWSER_EXTENSION_VERSION
    assert status["recommended_version"] == RECOMMENDED_BROWSER_EXTENSION_VERSION
    assert status["minimum_version"] == MIN_BROWSER_EXTENSION_VERSION
    assert status["needs_upgrade"] is True


def test_browser_status_accepts_current_extension_version():
    service = BrowserHandoffService()
    service.record_ping(RECOMMENDED_BROWSER_EXTENSION_VERSION)

    status = service.status()

    assert status["detected"] is True
    assert status["needs_upgrade"] is False
    assert status["message"] == "已连接 1 个浏览器插件"


def test_browser_status_keeps_upgrade_warning_for_idle_known_client():
    service = BrowserHandoffService(client_ttl=30)
    service.record_ping("2.0.9", "old-edge", "edge")
    service._clients["old-edge"].last_seen -= 31

    status = service.status()

    assert status["detected"] is False
    assert status["seen_before"] is True
    assert status["needs_upgrade"] is True
    assert status["release_url"] == BROWSER_EXTENSION_RELEASE_URL
    assert "此前连接" in status["message"]
    assert "建议升级" in status["message"]


def test_browser_status_does_not_downgrade_current_client_for_idle_old_history():
    service = BrowserHandoffService(client_ttl=30)
    service.record_ping("2.0.9", "old-edge", "edge")
    service._clients["old-edge"].last_seen -= 31
    service.record_ping(RECOMMENDED_BROWSER_EXTENSION_VERSION, "current-chrome", "chrome")

    status = service.status()

    assert status["detected"] is True
    assert status["version"] == RECOMMENDED_BROWSER_EXTENSION_VERSION
    assert status["needs_upgrade"] is False
    assert status["message"] == "已连接 1 个浏览器插件"
    old_client = next(item for item in status["clients"] if item["id"] == "old-edge")
    assert old_client["active"] is False
    assert old_client["needs_upgrade"] is True


def test_browser_status_keeps_multiple_client_versions_separate():
    service = BrowserHandoffService()
    service.record_ping("2.0.7", "edge-install", "edge")
    service.record_ping(RECOMMENDED_BROWSER_EXTENSION_VERSION, "firefox-install", "firefox")

    status = service.status()

    assert status["detected"] is True
    assert status["client_count"] == 2
    assert status["active_versions"] == [RECOMMENDED_BROWSER_EXTENSION_VERSION, "2.0.7"]
    assert {(item["browser"], item["version"]) for item in status["clients"]} == {
        ("edge", "2.0.7"),
        ("firefox", RECOMMENDED_BROWSER_EXTENSION_VERSION),
    }
    assert status["needs_upgrade"] is True


def test_browser_status_keeps_chromium_fork_identities_separate():
    service = BrowserHandoffService()
    for browser_name in ("chrome", "brave", "vivaldi", "opera"):
        service.record_ping(
            RECOMMENDED_BROWSER_EXTENSION_VERSION,
            f"{browser_name}-install",
            browser_name,
        )

    status = service.status()

    assert status["client_count"] == 4
    assert {item["browser"] for item in status["clients"]} == {
        "chrome", "brave", "vivaldi", "opera",
    }
    assert status["needs_upgrade"] is False


def test_task_cookie_uses_dpapi_on_windows():
    protected = protect_secret("session=secret")
    assert unprotect_secret(protected) == "session=secret"
    if protected != "session=secret":
        assert protected.startswith(PREFIX)


def test_native_host_manual_download_creates_task_immediately(monkeypatch):
    calls = []
    monkeypatch.setattr(native_host, "_ensure_app", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(
        native_host,
        "_request",
        lambda method, path, payload=None: calls.append((method, path, payload))
        or ({"id": "task-1"} if path == "/browser/downloads" else {"ok": True}),
    )

    result = native_host.dispatch(
        {"op": "download", "resource": {"url": "https://cdn.test/setup.exe"}}
    )

    assert result["task"]["id"] == "task-1"
    assert result["activated"] is False
    assert not any(path == "/app/activate" for _, path, _ in calls)
    assert ("POST", "/browser/downloads", {"url": "https://cdn.test/setup.exe"}) in calls


def test_native_host_ping_and_takeover_settings_share_desktop_source_of_truth(monkeypatch):
    calls = []
    monkeypatch.setattr(native_host, "_ensure_app", lambda *_args, **_kwargs: None)

    def request(method, path, payload=None, timeout=4):
        calls.append((method, path, payload))
        if path == "/browser/ping":
            return {
                "ok": True,
                "recommended_version": RECOMMENDED_BROWSER_EXTENSION_VERSION,
                "minimum_version": MIN_BROWSER_EXTENSION_VERSION,
                "release_url": BROWSER_EXTENSION_RELEASE_URL,
            }
        if path == "/health":
            return {"version": "1.4.1"}
        if path == "/settings" and method == "GET":
            return {"browser_takeover_enabled": False, "browser_takeover_min_mb": 3}
        if path == "/settings" and method == "POST":
            return {"browser_takeover_enabled": payload["browser_takeover_enabled"], "browser_takeover_min_mb": 3}
        return {"ok": True}

    monkeypatch.setattr(native_host, "_request", request)

    ping = native_host.dispatch({"op": "ping", "version": "1.4.1"})
    updated = native_host.dispatch({"op": "set_takeover_settings", "enabled": True})

    assert ping["takeover_enabled"] is False
    assert ping["takeover_minimum_bytes"] == 3 * 1024 * 1024
    assert ping["recommended_extension_version"] == RECOMMENDED_BROWSER_EXTENSION_VERSION
    assert ping["minimum_extension_version"] == MIN_BROWSER_EXTENSION_VERSION
    assert ping["extension_release_url"] == BROWSER_EXTENSION_RELEASE_URL
    assert updated["takeover_enabled"] is True
    assert ("POST", "/settings", {"browser_takeover_enabled": True}) in calls


def test_native_host_waits_for_handoff_with_one_long_request(monkeypatch):
    calls = []
    monkeypatch.setattr(native_host, "_ensure_app", lambda *_args, **_kwargs: None)

    def request(method, path, payload=None, timeout=4):
        calls.append((method, path, payload, timeout))
        if path.endswith("/wait"):
            return {"id": "handoff-1", "status": "accepted"}
        return {"ok": True}

    monkeypatch.setattr(native_host, "_request", request)
    result = native_host.dispatch({"op": "wait_handoff", "handoff_id": "handoff-1"})

    assert result["handoff"]["status"] == "accepted"
    waits = [call for call in calls if call[1].endswith("/wait")]
    assert waits == [("GET", "/browser/handoffs/handoff-1/wait", None, 125)]


def test_native_host_process_handles_multiple_messages(monkeypatch):
    messages = iter([{"op": "ping", "__request_id": "one"}, {"op": "ping"}, None])
    responses = []
    monkeypatch.setattr(native_host, "_read_message", lambda: next(messages))
    monkeypatch.setattr(native_host, "_write_message", responses.append)
    monkeypatch.setattr(native_host, "dispatch", lambda message: {"ok": True, "op": message["op"]})

    assert native_host.main() == 0
    assert responses == [
        {"ok": True, "op": "ping", "__request_id": "one"},
        {"ok": True, "op": "ping"},
    ]


def test_native_host_reads_a_message_split_across_pipe_reads():
    class SplitStream:
        def __init__(self, chunks):
            self.chunks = list(chunks)

        def read(self, length):
            if not self.chunks:
                return b""
            chunk = self.chunks.pop(0)
            if len(chunk) <= length:
                return chunk
            self.chunks.insert(0, chunk[length:])
            return chunk[:length]

    assert native_host._read_exact(SplitStream([b"a", b"bc", b"def"]), 6) == b"abcdef"


def test_native_host_waits_for_presenter_only_for_ui_operations(monkeypatch):
    requirements = []
    monkeypatch.setattr(
        native_host,
        "_ensure_app",
        lambda require_presenter=True: requirements.append(require_presenter),
    )
    monkeypatch.setattr(
        native_host,
        "_request",
        lambda method, path, payload=None, timeout=4: (
            {"recommended_version": "", "minimum_version": "", "release_url": ""}
            if path == "/browser/ping"
            else {"version": "3.0.8"}
            if path == "/health"
            else {"browser_takeover_enabled": True, "browser_takeover_min_mb": 0}
            if path == "/settings"
            else {"id": "handoff"}
        ),
    )

    native_host.dispatch({"op": "ping"})
    native_host.dispatch({"op": "handoff_status", "handoff_id": "handoff"})
    native_host.dispatch({"op": "offer", "resource": {"url": "https://cdn.test/a.mp4"}})

    assert requirements == [False, False, True]


def test_task_manager_finds_duplicate_urls():
    from backend.app.downloader.task_manager import TaskManager
    from backend.app.models import Task, TaskStatus, TaskType

    manager = TaskManager.__new__(TaskManager)
    manager.tasks = {}
    manager.tasks['a'] = Task(
        id='a',
        url='https://CDN.Example.com/video/File.mp4?x=1',
        task_type=TaskType.HTTP,
        status=TaskStatus.DONE,
        filename='File.mp4',
        updated_at='2026-01-02',
    )
    manager.tasks['b'] = Task(
        id='b',
        url='https://cdn.example.com/video/File.mp4?x=1',
        task_type=TaskType.HTTP,
        status=TaskStatus.DOWNLOADING,
        filename='File-copy.mp4',
        updated_at='2026-01-03',
    )
    matches = manager.find_tasks_by_url('https://cdn.example.com/video/File.mp4/?x=1')
    assert [item.id for item in matches] == ['b', 'a']
    assert manager.find_tasks_by_url('https://cdn.example.com/other.mp4') == []


def test_native_host_waits_for_presenter_after_cold_start(monkeypatch):
    calls = []
    health_hits = {'n': 0}

    def request(method, path, payload=None, timeout=4):
        calls.append((method, path))
        if path == '/health':
            health_hits['n'] += 1
            if health_hits['n'] == 1:
                raise RuntimeError('down')
            return {'ok': True, 'version': '1.3.3'}
        if path == '/browser/presenter':
            # First poll: session only; second poll: ready.
            ready_hits = sum(1 for item in calls if item[1] == '/browser/presenter')
            return {'ok': True, 'session': True, 'ready': ready_hits >= 2, 'mode': 'desktop' if ready_hits >= 2 else 'desktop-pending'}
        return {'ok': True}

    monkeypatch.setattr(native_host, '_request', request)
    monkeypatch.setattr(native_host, '_start_app', lambda: calls.append(('start', 'app')))
    monkeypatch.setattr(native_host.time, 'sleep', lambda _seconds: None)
    native_host._ensure_app()
    assert ('start', 'app') in calls
    assert any(path == '/browser/presenter' for _method, path in calls)


def test_browser_handoff_service_bounds_client_history_and_pending_items():
    service = BrowserHandoffService()
    for index in range(MAX_BROWSER_CLIENTS + 20):
        service.record_ping("3.0.8", f"client-{index}", "chrome")
    assert len(service._clients) == MAX_BROWSER_CLIENTS
    assert "client-0" not in service._clients

    for index in range(MAX_BROWSER_HANDOFFS + 20):
        service.create({"url": f"https://cdn.example.test/video-{index}.mp4"})
    assert len(service._items) == MAX_BROWSER_HANDOFFS


def test_removed_legacy_sniffed_endpoint_cannot_accumulate_payloads():
    from fastapi.testclient import TestClient
    from backend.app import api as api_module
    from backend.app.main import app

    original = api_module._check_token
    api_module._check_token = lambda _token: None
    try:
        with TestClient(app) as client:
            assert client.get("/api/sniffed", headers={"X-Token": "test"}).status_code == 404
            assert client.post("/api/sniffed", json={"value": "x"}, headers={"X-Token": "test"}).status_code == 404
    finally:
        api_module._check_token = original
