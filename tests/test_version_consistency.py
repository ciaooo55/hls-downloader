import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def _match(path: str, pattern: str) -> str:
    content = (ROOT / path).read_text(encoding="utf-8")
    result = re.search(pattern, content, flags=re.MULTILINE)
    assert result, f"version not found in {path}"
    return result.group(1)


def test_desktop_release_versions_are_consistent():
    version = _match("backend/app/version.py", r'APP_VERSION\s*=\s*"([^"]+)"')

    assert json.loads((ROOT / "frontend/package.json").read_text(encoding="utf-8"))["version"] == version
    assert json.loads((ROOT / "frontend/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))["version"] == version
    assert _match("frontend/src-tauri/Cargo.toml", r'^version\s*=\s*"([^"]+)"') == version
    assert _match(
        "frontend/src-tauri/Cargo.lock",
        r'name = "hls-downloader-desktop"\s+version = "([^"]+)"',
    ) == version
    assert _match("installer/hls-downloader.nsi", r'!define APP_VERSION\s+"([^"]+)"') == version
    assert _match("scripts/build_installer.ps1", r'\[string\]\$Version\s*=\s*"([^"]+)"') == version
    assert f'default: "{version}"' in (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")


def test_changed_extension_release_version_matches_desktop_recommendation():
    desktop_version = _match("backend/app/version.py", r'APP_VERSION\s*=\s*"([^"]+)"')
    extension_version = json.loads((ROOT / "extension/package.json").read_text(encoding="utf-8"))["version"]

    assert extension_version == desktop_version
    assert _match("extension/wxt.config.ts", r"extensionVersion\s*=.*?\|\|\s*'([^']+)'") == extension_version
    assert _match(
        "backend/app/browser_handoff.py",
        r'RECOMMENDED_BROWSER_EXTENSION_VERSION\s*=\s*"([^"]+)"',
    ) == extension_version
