import threading
import time

from fastapi.testclient import TestClient

from backend.app import desktop_runtime as runtime
from backend.app.config import settings
from backend.app.main import app
from backend.app.native_shell import (
    boot_native_shell,
    is_native_shell_ready,
    native_shell_supervisor,
    native_shell_was_closed,
    reset_native_shell,
    shutdown_native_shell,
)
from backend.app.desktop_runtime import (
    has_browser_handoff_presenter,
    present_browser_handoff,
    register_browser_handoff,
    set_desktop_handoff_session,
)


AUTH = {"X-Token": settings.token}


def test_present_prefers_native_shell_over_tauri_callback():
    shown: list[str] = []
    register_browser_handoff(shown.append)
    set_desktop_handoff_session(True)
    boot_native_shell()
    try:
        result = present_browser_handoff(
            "offer-native",
            snapshot={
                "id": "offer-native",
                "filename": "setup.exe",
                "url": "https://cdn.test/setup.exe",
                "size": 2048,
            },
        )
        time.sleep(0.05)
        assert result["mode"] == "native-shell"
        assert result["presented"] is True
        assert result["presentable"] is True
        assert result["snapshot"]["filename"] == "setup.exe"
        assert "cookie" not in result["snapshot"]
        assert shown == []
    finally:
        register_browser_handoff(None)
        set_desktop_handoff_session(False)
        reset_native_shell()


def test_present_keeps_tauri_queue_when_native_shell_is_not_booted():
    register_browser_handoff(None)
    set_desktop_handoff_session(True)
    try:
        assert is_native_shell_ready() is False
        queued = present_browser_handoff("queued-tauri")
        assert queued["mode"] == "desktop-pending"
        assert queued["queued"] is True
        fallback = present_browser_handoff("web-only")
        set_desktop_handoff_session(False)
        fallback = present_browser_handoff("web-only")
        assert fallback["mode"] == "ui-fallback"
    finally:
        register_browser_handoff(None)
        set_desktop_handoff_session(False)


def test_present_queues_until_native_shell_boots(monkeypatch):
    register_browser_handoff(None)
    set_desktop_handoff_session(False)
    reset_native_shell()
    monkeypatch.setenv("HLS_STARTED_BY_NATIVE_SHELL", "1")
    try:
        queued = present_browser_handoff(
            "pending-native",
            snapshot={
                "id": "pending-native",
                "filename": "pack.zip",
                "url": "https://cdn.test/pack.zip",
                "size": 8,
            },
        )
        assert queued["mode"] == "native-shell-pending"
        assert queued["queued"] is True
        assert is_native_shell_ready() is False
        client = TestClient(app)
        presenter = client.get("/api/browser/presenter", headers=AUTH).json()
        assert presenter["mode"] == "native-shell-pending"
        assert presenter["session"] is True
        assert presenter["ready"] is False
        boot_native_shell()
        events = native_shell_supervisor().wait_event(0, 0)["events"]
        handoff = next(item for item in events if item["kind"] == "handoff")
        assert handoff["snapshot"]["filename"] == "pack.zip"
        assert handoff["presentable"] is True
    finally:
        monkeypatch.delenv("HLS_STARTED_BY_NATIVE_SHELL", raising=False)
        reset_native_shell()
        register_browser_handoff(None)
        set_desktop_handoff_session(False)


def test_create_handoff_uses_native_shell_snapshot_without_tauri():
    from backend.app import api as api_module
    from backend.app.browser_handoff import browser_handoffs

    runtime.register_browser_handoff(None)
    runtime.set_desktop_handoff_session(False)
    boot_native_shell()
    client = TestClient(app)
    try:
        unauthorized = client.post("/api/desktop/native-shell/boot")
        assert unauthorized.status_code == 401

        response = client.post(
            "/api/browser/handoffs",
            json={
                "url": "https://cdn.example.test/setup.exe",
                "filename": "setup.exe",
                "title": "安装包",
                "size": 4096,
                "cookie": "must-not-paint",
            },
            headers=AUTH,
        )
        assert response.status_code == 200
        body = response.json()
        assert body["presentation_mode"] == "native-shell"
        assert body["presentation"] == "presented"
        assert body["presented"] is True
        assert body["presentable"] is True
        assert body["snapshot"]["filename"] == "setup.exe"
        assert body["snapshot"]["url"] == "https://cdn.example.test/setup.exe"
        assert body["snapshot"]["size"] == 4096
        assert "cookie" not in body["snapshot"]
        stored = browser_handoffs.get(body["id"])
        assert stored is not None
        assert stored.presented is True

        presenter = client.get("/api/browser/presenter", headers=AUTH)
        assert presenter.status_code == 200
        assert presenter.json() == {
            "ok": True,
            "ready": True,
            "session": True,
            "mode": "native-shell",
        }

        events = client.get(
            "/api/desktop/native-shell/events",
            params={"after": 0, "timeout": 0.2},
            headers=AUTH,
        )
        assert events.status_code == 200
        kinds = [item["kind"] for item in events.json()["events"]]
        assert "handoff" in kinds
        assert api_module.is_native_shell_ready() is True
    finally:
        runtime.register_browser_handoff(None)
        runtime.set_desktop_handoff_session(False)


def test_hide_main_does_not_stop_resident_shell():
    client = TestClient(app)
    boot = client.post("/api/desktop/native-shell/boot", headers=AUTH)
    assert boot.status_code == 200
    assert boot.json()["main_open"] is False
    opened = client.post("/api/desktop/native-shell/main/open", headers=AUTH)
    assert opened.json()["main_open"] is True
    hidden = client.post("/api/desktop/native-shell/main/hide", headers=AUTH)
    body = hidden.json()
    assert body["resident"] is True
    assert body["main_open"] is False
    assert body["windows"]["handoff"] is True
    presenter = client.get("/api/browser/presenter", headers=AUTH)
    assert presenter.json()["mode"] == "native-shell"


def test_native_shell_settings_activates_existing_desktop_session():
    client = TestClient(app)
    client.post("/api/desktop/native-shell/boot", headers=AUTH)
    missing = client.post("/api/desktop/native-shell/settings", headers=AUTH)
    assert missing.status_code == 200
    assert missing.json()["ok"] is False
    client.post("/api/desktop/session/start", headers=AUTH)
    try:
        shown = client.post("/api/desktop/native-shell/settings", headers=AUTH)
        assert shown.status_code == 200
        assert shown.json()["ok"] is True
        commands = client.get("/api/desktop/session/commands?after=0&timeout=0", headers=AUTH)
        kinds = [item["kind"] for item in commands.json()["commands"]]
        assert "activate" in kinds
    finally:
        client.post("/api/desktop/session/stop", headers=AUTH)


def test_native_shell_progress_complete_and_ipc_require_boot_and_token():
    client = TestClient(app)
    missing = client.post("/api/desktop/native-shell/progress", json={"tasks": []}, headers=AUTH)
    assert missing.status_code == 409
    client.post("/api/desktop/native-shell/boot", headers=AUTH)
    progress = client.post(
        "/api/desktop/native-shell/progress",
        json={"tasks": [{"id": "t1", "filename": "a.bin", "percent": 40}]},
        headers=AUTH,
    )
    complete = client.post(
        "/api/desktop/native-shell/complete",
        json={"item": {"id": "t1", "filename": "a.bin"}},
        headers=AUTH,
    )
    assert progress.status_code == 200
    assert complete.json()["presentable"] is True
    ipc = client.post("/api/desktop/native-shell/ipc/start", headers=AUTH)
    assert ipc.status_code == 200
    assert ipc.json()["port"] > 0
    assert ipc.json()["host"] == "127.0.0.1"
    stopped = client.post("/api/desktop/native-shell/ipc/stop", headers=AUTH)
    assert stopped.json() == {"ok": True}


def test_native_shell_does_not_bypass_host_or_legal_checks(monkeypatch):
    from backend.app import api as api_module
    from backend.app import legal as legal_module

    client = TestClient(app)
    client.post("/api/desktop/native-shell/boot", headers=AUTH)

    monkeypatch.setattr(api_module.settings, "allowed_hosts", ["only.example.test"])
    blocked = client.post(
        "/api/browser/handoffs",
        json={"url": "https://cdn.example.test/setup.exe", "filename": "setup.exe"},
        headers=AUTH,
    )
    assert blocked.status_code == 403

    current = legal_module.settings
    current.legal_terms_accepted_version = ""
    current.legal_terms_accepted_digest = ""
    current.legal_terms_accepted_at = ""
    monkeypatch.setattr(api_module.settings, "allowed_hosts", [])
    gated = client.post(
        "/api/browser/handoffs",
        json={"url": "https://cdn.example.test/setup.exe", "filename": "setup.exe"},
        headers=AUTH,
    )
    assert gated.status_code == 428
    assert gated.json()["detail"]["code"] == "LEGAL_TERMS_REQUIRED"


def test_create_handoff_still_queues_tauri_when_shell_not_booted(monkeypatch):
    from backend.app import api as api_module

    runtime.register_browser_handoff(None)
    runtime.set_desktop_handoff_session(True)
    monkeypatch.setattr(api_module, "_check_token", lambda _token: None)
    monkeypatch.setattr(api_module, "_check_host", lambda _url: None)
    try:
        client = TestClient(app)
        response = client.post(
            "/api/browser/handoffs",
            json={"url": "https://cdn.example.test/clip.mp4", "filename": "clip.mp4"},
            headers={"X-Token": "test"},
        )
        assert response.status_code == 200
        body = response.json()
        assert body["presentation_mode"] == "desktop-pending"
        assert body["presentation_queued"] is True
        assert body["presentable"] is False
    finally:
        runtime.set_desktop_handoff_session(False)


def test_http_offer_wakes_precreated_window_under_200ms():
    boot_native_shell()
    result = {}

    def wait():
        result.update(native_shell_supervisor().wait_event(0, 2))

    worker = threading.Thread(target=wait)
    worker.start()
    time.sleep(0.02)
    started = time.monotonic()
    client = TestClient(app)
    response = client.post(
        "/api/browser/handoffs",
        json={"url": "https://cdn.example.test/a.mp4", "filename": "a.mp4"},
        headers=AUTH,
    )
    worker.join(2)
    elapsed_ms = (time.monotonic() - started) * 1000
    assert response.status_code == 200
    assert response.json()["presentation_mode"] == "native-shell"
    assert response.json()["snapshot"]["filename"] == "a.mp4"
    assert not worker.is_alive()
    assert result["events"][0]["kind"] == "handoff"
    assert result["events"][0]["presentable"] is True
    assert elapsed_ms < 200


def test_has_tauri_presenter_is_unchanged_when_shell_idle():
    register_browser_handoff(lambda _handoff_id: None)
    try:
        assert has_browser_handoff_presenter() is True
        assert is_native_shell_ready() is False
        client = TestClient(app)
        body = client.get("/api/browser/presenter", headers=AUTH).json()
        assert body["mode"] == "desktop"
        assert body["ready"] is True
    finally:
        register_browser_handoff(None)
        set_desktop_handoff_session(False)


def test_present_does_not_stay_pending_after_tray_shutdown(monkeypatch):
    register_browser_handoff(None)
    set_desktop_handoff_session(False)
    reset_native_shell()
    monkeypatch.setenv("HLS_STARTED_BY_NATIVE_SHELL", "1")
    boot_native_shell()
    try:
        assert is_native_shell_ready() is True
        shutdown_native_shell()
        assert is_native_shell_ready() is False
        assert native_shell_was_closed() is True
        client = TestClient(app)
        presenter = client.get("/api/browser/presenter", headers=AUTH).json()
        assert presenter["mode"] not in {"native-shell", "native-shell-pending"}
        queued = present_browser_handoff(
            "after-exit",
            snapshot={
                "id": "after-exit",
                "filename": "setup.exe",
                "url": "https://cdn.test/setup.exe",
                "size": 8,
            },
        )
        assert queued["mode"] == "ui-fallback"
        assert queued["queued"] is False
    finally:
        monkeypatch.delenv("HLS_STARTED_BY_NATIVE_SHELL", raising=False)
        reset_native_shell()
        register_browser_handoff(None)
        set_desktop_handoff_session(False)


def test_closed_shell_force_respawns_on_next_offer(monkeypatch, tmp_path):
    from backend.app import native_shell as ns

    captured = {}

    class FakeProcess:
        pass

    def fake_popen(*args, **kwargs):
        captured["args"] = args
        captured["kwargs"] = kwargs
        return FakeProcess()

    exe = tmp_path / "HLSNativeShell.exe"
    exe.write_bytes(b"MZ")
    monkeypatch.setattr(ns, "running_on_windows", lambda: True)
    monkeypatch.delenv("PYTEST_CURRENT_TEST", raising=False)
    monkeypatch.setenv("HLS_STARTED_BY_NATIVE_SHELL", "1")
    monkeypatch.setattr(ns, "locate_native_shell_executable", lambda project_root=None: exe)
    monkeypatch.setattr(ns.subprocess, "Popen", fake_popen)
    register_browser_handoff(None)
    set_desktop_handoff_session(False)
    reset_native_shell()
    boot_native_shell()
    try:
        shutdown_native_shell()
        assert is_native_shell_ready() is False
        queued = present_browser_handoff(
            "respawn-native",
            snapshot={
                "id": "respawn-native",
                "filename": "pack.zip",
                "url": "https://cdn.test/pack.zip",
                "size": 8,
            },
        )
        assert queued["mode"] == "native-shell-pending"
        assert queued["queued"] is True
        assert captured.get("args")
        assert captured["args"][0][0] == str(exe)
        flags = captured["kwargs"]["creationflags"]
        assert flags & 0x08000000 == 0
        client = TestClient(app)
        presenter = client.get("/api/browser/presenter", headers=AUTH).json()
        assert presenter["mode"] == "native-shell-pending"
    finally:
        monkeypatch.delenv("HLS_STARTED_BY_NATIVE_SHELL", raising=False)
        reset_native_shell()
        register_browser_handoff(None)
        set_desktop_handoff_session(False)
