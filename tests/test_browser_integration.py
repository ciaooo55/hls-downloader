import time

from backend.app.browser_handoff import BrowserHandoffService
from backend.app.credentials import PREFIX, protect_secret, unprotect_secret
from backend import native_host


def test_browser_handoff_confirmation_and_expiry():
    service = BrowserHandoffService(ttl=0.01)
    item = service.create({"url": "https://cdn.test/file.zip", "cookie": "session=secret", "size": 42})
    assert item.status == "pending"
    assert "cookie" not in item.public()
    assert service.pending()[0]["id"] == item.id
    assert service.reject(item.id).status == "rejected"

    expired = service.create({"url": "https://cdn.test/old.zip"})
    expired.created_at = time.time() - 0.02
    assert service.get(expired.id).status == "expired"


def test_browser_status_explains_when_extension_has_never_connected():
    service = BrowserHandoffService()

    assert service.status() == {
        "detected": False,
        "seen_before": False,
        "version": "",
        "state": "not_detected",
        "message": "未检测到浏览器扩展；浏览器下载不会被接管",
    }


def test_task_cookie_uses_dpapi_on_windows():
    protected = protect_secret("session=secret")
    assert unprotect_secret(protected) == "session=secret"
    if protected != "session=secret":
        assert protected.startswith(PREFIX)


def test_native_host_manual_download_creates_task_immediately(monkeypatch):
    calls = []
    monkeypatch.setattr(native_host, "_ensure_app", lambda: None)
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
    assert result["activated"] is True
    assert ("POST", "/browser/downloads", {"url": "https://cdn.test/setup.exe"}) in calls
    assert ("POST", "/app/activate", {}) in calls


def test_native_host_waits_for_handoff_with_one_long_request(monkeypatch):
    calls = []
    monkeypatch.setattr(native_host, "_ensure_app", lambda: None)

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
    messages = iter([{"op": "ping"}, {"op": "ping"}, None])
    responses = []
    monkeypatch.setattr(native_host, "_read_message", lambda: next(messages))
    monkeypatch.setattr(native_host, "_write_message", responses.append)
    monkeypatch.setattr(native_host, "dispatch", lambda message: {"ok": True, "op": message["op"]})

    assert native_host.main() == 0
    assert responses == [{"ok": True, "op": "ping"}, {"ok": True, "op": "ping"}]


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
            return {'ok': True, 'version': '1.3.2'}
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
