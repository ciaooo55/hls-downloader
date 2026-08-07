from backend.app.access_tokens import (
    issue_browser_access_token,
    issue_desktop_access_token,
    issue_file_access_token,
    verify_browser_access_token,
    verify_desktop_access_token,
    verify_file_access_token,
)
from fastapi.testclient import TestClient

from backend.app.main import app


def test_file_access_token_is_scoped_and_expires(monkeypatch):
    from backend.app import access_tokens

    monkeypatch.setattr(access_tokens.settings, "token", "desktop-control-secret")
    token = issue_file_access_token("task-one", now=1000)

    assert "desktop-control-secret" not in token
    assert verify_file_access_token("task-one", token, now=1001) is True
    assert verify_file_access_token("task-two", token, now=1001) is False
    assert verify_file_access_token("task-one", token, now=2000) is False


def test_desktop_and_browser_credentials_are_short_lived_and_scope_bound(monkeypatch):
    from backend.app import access_tokens

    monkeypatch.setattr(access_tokens.settings, "token", "desktop-control-secret")
    desktop = issue_desktop_access_token(now=1000)
    browser = issue_browser_access_token(now=1000)

    assert "desktop-control-secret" not in desktop
    assert "desktop-control-secret" not in browser
    assert verify_desktop_access_token(desktop, now=1001)
    assert not verify_browser_access_token(desktop, now=1001)
    assert verify_browser_access_token(browser, now=1001)
    assert not verify_desktop_access_token(browser, now=1001)


def test_browser_credential_cannot_access_desktop_control_routes():
    browser = issue_browser_access_token()
    desktop = issue_desktop_access_token()
    with TestClient(app) as client:
        ping = client.post(
            "/api/browser/ping",
            headers={"X-Token": browser},
            json={"version": "test", "client_id": "scope-test", "browser": "chrome"},
        )
        blocked_settings = client.get("/api/settings", headers={"X-Token": browser})
        desktop_settings = client.get("/api/settings", headers={"X-Token": desktop})
        blocked_shutdown = client.post(
            "/api/desktop/core/shutdown",
            headers={"X-Token": desktop},
        )

    assert ping.status_code == 200
    assert blocked_settings.status_code == 401
    assert desktop_settings.status_code == 200
    assert blocked_shutdown.status_code == 401


def test_core_shutdown_marks_running_tasks_for_resume(monkeypatch):
    from backend.app import api as api_module

    async def mark():
        return 2

    monkeypatch.setattr(api_module.manager, "prepare_for_update_restart", mark)
    monkeypatch.setattr(api_module, "request_core_shutdown", lambda: True)
    with TestClient(app) as client:
        response = client.post(
            "/api/desktop/core/shutdown",
            headers={"X-Token": api_module.settings.token},
        )
    assert response.status_code == 200
    assert response.json() == {"ok": True, "resume_tasks": 2}
