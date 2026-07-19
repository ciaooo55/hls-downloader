from fastapi.testclient import TestClient
from pathlib import Path
from datetime import datetime, timedelta, timezone

from backend.app import userscript_monitor as monitor_module
from backend.app.main import app
from backend.app.userscript_monitor import UserscriptMonitor


TOKEN_HEADERS = {"X-Token": "55555"}


def test_bundled_userscript_is_served_from_install_url():
    with TestClient(app) as client:
        response = client.get("/userscript/m3u8-sniffer.user.js")

    assert response.status_code == 200
    assert "// ==UserScript==" in response.text
    assert "m3u8 一键下载" in response.text


def test_userscript_ping_updates_authenticated_status_and_help_page():
    with TestClient(app) as client:
        ping = client.post(
            "/api/userscript/ping",
            headers=TOKEN_HEADERS,
            json={
                "version": "4.0.0",
                "page_url": "https://video.example/watch/secret?id=42",
            },
        )
        status = client.get("/api/userscript/status", headers=TOKEN_HEADERS)
        help_page = client.get("/help")

    assert ping.status_code == 200
    assert ping.json() == {"ok": True}
    assert status.status_code == 200
    assert status.json()["detected"] is True
    assert status.json()["seen_before"] is True
    assert status.json()["version"] == "4.0.0"
    assert status.json()["page_origin"] == "https://video.example"
    assert status.json()["last_seen_at"]
    assert help_page.status_code == 200
    assert "已检测到浏览器脚本运行" in help_page.text
    assert "/userscript/m3u8-sniffer.user.js" in help_page.text


def test_userscript_status_endpoints_require_token():
    with TestClient(app) as client:
        ping = client.post("/api/userscript/ping", json={"version": "4.0.0"})
        status = client.get("/api/userscript/status")

    assert ping.status_code == 401
    assert status.status_code == 401


def test_userscript_sends_startup_ping_and_periodic_heartbeat():
    root = Path(__file__).resolve().parent.parent
    source = (root / "userscript" / "m3u8-sniffer.user.js").read_text(encoding="utf-8")

    assert "const SCRIPT_VERSION = '4.3.0';" in source
    assert "// @compatible   Tampermonkey" in source
    assert "// @compatible   ScriptCat" in source
    assert "apiPost('/userscript/ping'" in source
    assert "page_url: location.href" in source
    assert "setTimeout(pingDownloader, 500);" in source
    assert "setInterval(pingDownloader, 60000);" in source


def test_userscript_panel_is_collapsible_and_remembers_its_position():
    root = Path(__file__).resolve().parent.parent
    source = (root / "userscript" / "m3u8-sniffer.user.js").read_text(encoding="utf-8")

    assert "GM_getValue('hls_panel_collapsed', true)" in source
    assert "GM_setValue('hls_panel_collapsed'" in source
    assert "GM_setValue('hls_panel_side'" in source
    assert "hls-collapsed" in source
    assert "data-tab=\"resources\"" in source
    assert "data-tab=\"tasks\"" in source


def test_userscript_exposes_common_resource_and_task_actions():
    root = Path(__file__).resolve().parent.parent
    source = (root / "userscript" / "m3u8-sniffer.user.js").read_text(encoding="utf-8")

    for marker in (
        "hls-download-all",
        "hls-rescan",
        "actionButton('pause'",
        "actionButton('resume'",
        "actionButton('cancel'",
        "actionButton('retry'",
        "actionButton('launch'",
    ):
        assert marker in source
    assert "escapeHTML(state.error)" in source


def test_userscript_uses_current_page_as_request_source_for_cross_domain_media():
    root = Path(__file__).resolve().parent.parent
    source = (root / "userscript" / "m3u8-sniffer.user.js").read_text(encoding="utf-8")

    assert "referer: location.href" in source
    assert "origin: location.origin" in source
    assert "referer: 'https://missav.ai/'" not in source
    assert "origin: 'https://missav.ai'" not in source


def test_packaged_entry_starts_desktop_window_without_external_browser():
    root = Path(__file__).resolve().parent.parent
    source = (root / "backend" / "run_server.py").read_text(encoding="utf-8")

    assert "from desktop import main" in source
    assert "webbrowser" not in source
    assert "main()" in source


def test_monitor_marks_an_old_heartbeat_as_not_current(monkeypatch):
    class FrozenDateTime(datetime):
        current = datetime(2026, 7, 13, 12, 0, tzinfo=timezone.utc)

        @classmethod
        def now(cls, tz=None):
            return cls.current

    monkeypatch.setattr(monitor_module, "datetime", FrozenDateTime)
    monitor = UserscriptMonitor(freshness_seconds=150)
    monitor.record(version="4.0.0", page_url="https://video.example/watch")
    assert monitor.snapshot().detected is True

    FrozenDateTime.current += timedelta(seconds=151)
    snapshot = monitor.snapshot()
    assert snapshot.detected is False
    assert snapshot.seen_before is True


def test_help_page_escapes_userscript_metadata():
    marker = "<script>alert('x')</script>"
    with TestClient(app) as client:
        ping = client.post(
            "/api/userscript/ping",
            headers=TOKEN_HEADERS,
            json={"version": marker, "page_url": "https://video.example/watch"},
        )
        help_page = client.get("/help")

    assert ping.status_code == 200
    assert marker not in help_page.text
    assert "&lt;script&gt;" in help_page.text
