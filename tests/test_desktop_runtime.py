import threading
from pathlib import Path

from fastapi.testclient import TestClient

from backend.app import main as main_module
from backend.app.config import settings
from backend.app.desktop_runtime import (
    activate_window,
    has_browser_handoff_presenter,
    is_desktop_handoff_session,
    present_browser_handoff,
    register_activation,
    register_browser_handoff,
    register_shutdown,
    request_shutdown,
    set_desktop_handoff_session,
)
from backend.app.main import app


def test_activation_returns_without_waiting_for_blocked_window():
    started = threading.Event()
    release = threading.Event()

    def blocked_activation():
        started.set()
        release.wait(timeout=2)

    register_activation(blocked_activation)
    try:
        assert activate_window() is True
        assert started.wait(timeout=1.0)
        assert activate_window() is True
    finally:
        release.set()
        register_activation(None)


def test_activation_reports_false_when_no_window_is_registered():
    register_activation(None)
    assert activate_window() is False


def test_registered_shutdown_requests_controller_exit():
    calls: list[str] = []
    register_shutdown(lambda: calls.append("exit"))
    try:
        assert request_shutdown() is True
        assert calls == ["exit"]
    finally:
        register_shutdown(None)


def test_registered_shutdown_returns_callback_result():
    register_shutdown(lambda: False)
    try:
        assert request_shutdown() is False
    finally:
        register_shutdown(None)


def test_activation_api_requires_token_and_calls_registered_window():
    calls: list[str] = []
    register_activation(lambda: calls.append("activate"))
    try:
        with TestClient(app) as client:
            unauthorized = client.post("/api/app/activate")
            activated = client.post(
                "/api/app/activate", headers={"X-Token": settings.token}
            )

        assert unauthorized.status_code == 401
        assert activated.status_code == 200
        assert activated.json() == {"ok": True}
        assert calls == ["activate"]
    finally:
        register_activation(None)


def test_shutdown_api_requires_token_and_calls_registered_shutdown():
    calls: list[str] = []
    register_shutdown(lambda: calls.append("shutdown"))
    try:
        with TestClient(app) as client:
            unauthorized = client.post("/api/app/shutdown")
            stopped = client.post(
                "/api/app/shutdown", headers={"X-Token": settings.token}
            )

        assert unauthorized.status_code == 401
        assert stopped.status_code == 200
        assert stopped.json() == {"ok": True, "resume_tasks": 0}
        assert calls == ["shutdown"]
    finally:
        register_shutdown(None)


def test_ui_files_disable_persistent_webview_cache(monkeypatch, tmp_path: Path):
    dist = tmp_path / "dist"
    dist.mkdir()
    (dist / "index.html").write_text("<h1>fresh UI</h1>", encoding="utf-8")
    (dist / "app.js").write_text("console.log('fresh')", encoding="utf-8")
    monkeypatch.setattr(main_module, "UI_DIST", dist)

    with TestClient(app) as client:
        root = client.get("/ui?version=9.8.7", follow_redirects=False)
        index = client.get("/ui/?version=9.8.7")
        asset = client.get("/ui/app.js")

    assert root.status_code == 307
    assert root.headers["location"] == "/ui/"
    assert index.status_code == 200
    assert asset.status_code == 200
    assert "no-store" in index.headers["cache-control"]
    assert "no-store" in asset.headers["cache-control"]


def test_ui_files_reject_windows_and_posix_path_escape(monkeypatch, tmp_path: Path):
    dist = tmp_path / "frontend" / "dist"
    dist.mkdir(parents=True)
    (dist / "index.html").write_text("<h1>safe UI</h1>", encoding="utf-8")
    secret = tmp_path / "secret.txt"
    secret.write_text("must-not-leak", encoding="utf-8")
    monkeypatch.setattr(main_module, "UI_DIST", dist)

    for value in (
        "../secret.txt",
        "..\\..\\secret.txt",
        r"C:\Windows\win.ini",
        r"\\server\share\secret.txt",
        "/etc/passwd",
    ):
        assert main_module._resolve_ui_file(value) is None

    with TestClient(app) as client:
        responses = [
            client.get("/ui/%2e%2e/secret.txt"),
            client.get("/ui/..%5c..%5csecret.txt"),
            client.get("/ui/C:%5cWindows%5cwin.ini"),
            client.get("/ui/%5c%5cserver%5cshare%5csecret.txt"),
        ]

    assert all(response.status_code == 404 for response in responses)
    assert all("must-not-leak" not in response.text for response in responses)


def test_cors_rejects_unrelated_extensions_and_random_local_origins():
    with TestClient(app) as client:
        rejected = client.options(
            "/api/health",
            headers={
                "Origin": "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "Access-Control-Request-Method": "GET",
            },
        )
        random_local = client.options(
            "/api/health",
            headers={
                "Origin": "http://127.0.0.1:45678",
                "Access-Control-Request-Method": "GET",
            },
        )
        allowed = client.options(
            "/api/health",
            headers={
                "Origin": main_module.CHROMIUM_EXTENSION_ORIGIN,
                "Access-Control-Request-Method": "GET",
            },
        )

    assert "access-control-allow-origin" not in rejected.headers
    assert "access-control-allow-origin" not in random_local.headers
    assert allowed.headers["access-control-allow-origin"] == (
        main_module.CHROMIUM_EXTENSION_ORIGIN
    )


def test_browser_handoffs_queue_during_cold_start_and_flush_independently():
    shown: list[str] = []
    ready = threading.Event()
    register_browser_handoff(None)
    set_desktop_handoff_session(False)
    try:
        fallback = present_browser_handoff("web-only")
        assert fallback["mode"] == "ui-fallback"
        assert fallback["queued"] is False

        set_desktop_handoff_session(True)
        queued_one = present_browser_handoff("queued-one")
        queued_two = present_browser_handoff("queued-two")
        assert queued_one["ok"] is True and queued_one["queued"] is True
        assert queued_two["mode"] == "desktop-pending"

        def show(handoff_id: str) -> None:
            shown.append(handoff_id)
            if {"queued-one", "queued-two"}.issubset(shown):
                ready.set()

        register_browser_handoff(show)
        assert ready.wait(timeout=1)
        assert {"queued-one", "queued-two"}.issubset(shown)
    finally:
        register_browser_handoff(None)
        set_desktop_handoff_session(False)


def test_presenter_status_endpoint_reflects_desktop_session():
    register_browser_handoff(None)
    set_desktop_handoff_session(False)
    try:
        with TestClient(app) as client:
            idle = client.get(
                "/api/browser/presenter", headers={"X-Token": settings.token}
            )
            assert idle.status_code == 200
            assert idle.json()["ready"] is False
            assert idle.json()["session"] is False
            assert idle.json()["mode"] == "ui-fallback"

            set_desktop_handoff_session(True)
            pending = client.get(
                "/api/browser/presenter", headers={"X-Token": settings.token}
            )
            body = pending.json()
            assert body["session"] is True
            assert body["ready"] is False
            assert body["mode"] == "desktop-pending"

            register_browser_handoff(lambda _handoff_id: None)
            ready = client.get(
                "/api/browser/presenter", headers={"X-Token": settings.token}
            )
            ready_body = ready.json()
            assert ready_body["ready"] is True
            assert ready_body["mode"] == "desktop"
            assert has_browser_handoff_presenter() is True
            assert is_desktop_handoff_session() is True
    finally:
        register_browser_handoff(None)
        set_desktop_handoff_session(False)
