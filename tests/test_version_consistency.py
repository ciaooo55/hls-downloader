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


def test_v6_crate_versions_stay_on_the_preview_line():
    version = _match("native_ui/Cargo.toml", r'^version\s*=\s*"([^"]+)"')
    assert version.startswith("6.")
    assert _match("native_shell/Cargo.toml", r'^version\s*=\s*"([^"]+)"') == version
    assert _match("installer/hls-downloader-v6.nsi", r'!define APP_VERSION\s+"([^"]+)"') == version
    assert _match("scripts/build_v6.ps1", r'\[string\]\$Version\s*=\s*"([^"]+)"') == version


def test_extension_version_matches_desktop_recommendation():
    extension_version = json.loads((ROOT / "extension/package.json").read_text(encoding="utf-8"))["version"]

    assert _match("extension/wxt.config.ts", r"extensionVersion\s*=.*?\|\|\s*'([^']+)'") == extension_version
    assert _match(
        "backend/app/browser_handoff.py",
        r'RECOMMENDED_BROWSER_EXTENSION_VERSION\s*=\s*"([^"]+)"',
    ) == extension_version
