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
    assert ("POST", "/browser/downloads", {"url": "https://cdn.test/setup.exe"}) in calls
