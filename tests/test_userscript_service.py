from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from backend.app.main import app
from backend.app.userscript_service import export_userscript, render_userscript


SOURCE = """// ==UserScript==
// @version      4.0.0
  const API_BASE = 'http://127.0.0.1:8765/api';
  const TOKEN = '55555'; // Must match config.json token
  const SCRIPT_VERSION = '4.0.0';
"""


def test_render_injects_current_api_token_and_version():
    rendered = render_userscript(
        SOURCE,
        host="127.0.0.1",
        port=9000,
        token="a'b\\c",
        version="4.2.0",
    )

    assert 'const API_BASE = "http://127.0.0.1:9000/api";' in rendered
    assert 'const TOKEN = "a\'b\\\\c";' in rendered
    assert 'const SCRIPT_VERSION = "4.2.0";' in rendered
    assert "// @version      4.2.0" in rendered


def test_export_is_atomic_and_refuses_unconfirmed_overwrite(tmp_path):
    target = export_userscript(tmp_path, "first", overwrite=False)
    assert target.read_text(encoding="utf-8") == "first"

    with pytest.raises(FileExistsError):
        export_userscript(tmp_path, "second", overwrite=False)

    assert target.read_text(encoding="utf-8") == "first"
    assert list(tmp_path.glob("*.tmp")) == []


def test_export_requires_existing_directory(tmp_path):
    with pytest.raises(NotADirectoryError):
        export_userscript(tmp_path / "missing", "script", overwrite=False)


def test_install_route_renders_current_settings_without_cache(monkeypatch):
    from backend.app import main as main_module

    monkeypatch.setattr(main_module.settings, "port", 9123)
    monkeypatch.setattr(main_module.settings, "token", "changed-token")
    with TestClient(app) as client:
        response = client.get("/userscript/m3u8-sniffer.user.js")

    assert response.status_code == 200
    assert 'const API_BASE = "http://127.0.0.1:9123/api";' in response.text
    assert 'const TOKEN = "changed-token";' in response.text
    assert response.headers["cache-control"] == "no-store"
