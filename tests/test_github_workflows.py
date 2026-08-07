from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent


def _workflow(name: str) -> str:
    path = ROOT / ".github" / "workflows" / name
    assert path.exists(), f"missing workflow: {path}"
    return path.read_text(encoding="utf-8")


def test_ci_runs_windows_python_and_frontend_checks():
    workflow = _workflow("ci.yml")

    assert "push:" in workflow
    assert "pull_request:" in workflow
    assert "windows-latest" in workflow
    assert "ubuntu-latest" not in workflow
    assert "python -m pytest -q" in workflow
    assert "python -m ruff check" in workflow
    assert "python -m mypy" in workflow
    assert "--cov-fail-under=60" in workflow
    assert "pnpm test" in workflow
    assert "pnpm run build" in workflow
    assert "working-directory: extension" in workflow
    assert "web-ext lint --source-dir .output/firefox-mv3 --warnings-as-errors" in workflow
    assert "dtolnay/rust-toolchain@4cda84d5c5c54efe2404f9d843567869ab1699d4" in workflow
    assert "pnpm run tauri:build" in workflow
    assert "cargo check --locked --all-targets" in workflow
    assert "cargo clippy --locked --all-targets -- -D warnings" in workflow
    assert "browser-actions/setup-chrome@2e1d749697dd1612b833dba4a722266286fbefcd" in workflow
    assert "browser-actions/setup-firefox@0bc507ddf224827e3b1af68e014d5e42ab93e795" in workflow
    assert "scripts/smoke_extension_browsers.py" in workflow
    assert "scripts/smoke_extension_takeover.py" in workflow
    assert "actions/setup-java@v5" not in workflow
    assert "permissions:\n  contents: read" in workflow


def test_release_builds_only_windows_assets_and_publishes_tags():
    workflow = _workflow("release.yml")

    assert "workflow_dispatch:" in workflow
    assert 'default: "3.0.18"' in workflow
    assert "include_extensions:" in workflow
    assert "Upload browser extension ZIPs for this release" in workflow
    assert "tags:" in workflow and "v*" in workflow
    assert "windows-latest" in workflow
    assert "dtolnay/rust-toolchain@4cda84d5c5c54efe2404f9d843567869ab1699d4" in workflow
    assert "actions/setup-java@v5" not in workflow
    assert "ubuntu-latest" not in workflow
    assert "choco install ffmpeg nsis" not in workflow
    assert "Build Windows packages" in workflow
    assert "scripts\\build_installer.ps1" in workflow
    assert '$buildArgs = @{ Version = $version }' in workflow
    assert "$buildArgs.IncludeExtensionAssets = $true" in workflow
    assert '$buildArgs = @("-Version", $version)' not in workflow
    assert "$prefix-Windows-x64-Setup.exe" in workflow
    assert "$prefix-Windows-x64-Portable.zip" in workflow
    assert "m3u8-sniffer.user.js" not in workflow
    assert "steps.extension-assets.outputs.include_extensions" in workflow
    assert "git diff --name-only $previousTag $env:GITHUB_REF_NAME -- extension" in workflow
    assert "$prefix-Firefox-Unsigned.zip" in workflow
    assert "$prefix-Firefox-Source.zip" in workflow
    assert "web-ext sign" not in workflow
    assert "--channel unlisted" not in workflow
    assert "web-ext lint --source-dir .output/firefox-mv3 --warnings-as-errors" in workflow
    assert "python -m ruff check" in workflow
    assert "python -m mypy" in workflow
    assert "--cov-fail-under=60" in workflow
    assert "cargo check --locked --all-targets" in workflow
    assert "cargo clippy --locked --all-targets -- -D warnings" in workflow
    assert "browser-actions/setup-chrome@2e1d749697dd1612b833dba4a722266286fbefcd" in workflow
    assert "browser-actions/setup-firefox@0bc507ddf224827e3b1af68e014d5e42ab93e795" in workflow
    assert "scripts/smoke_extension_browsers.py" in workflow
    assert "scripts/smoke_extension_takeover.py" in workflow
    assert "scripts/smoke_real_download.py --archive $archive" in workflow
    assert "scripts/smoke-portable-upgrade.ps1" in workflow
    assert "scripts/smoke-installer-upgrade.ps1" in workflow
    assert "SHA256SUMS.txt" not in workflow
    assert "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a" in workflow
    assert "actions/download-artifact@37930b1c2abaa49bbe596cd826c3c89aef350131" in workflow
    assert "softprops/action-gh-release" not in workflow
    assert "gh release create" in workflow
    assert "gh release upload" in workflow
    assert "--clobber" in workflow
    assert 'docs\\releases' in workflow
    assert '"--notes-file"' in workflow
    assert '"--generate-notes"' in workflow
    assert '--repo "${{ github.repository }}"' in workflow
    assert "startsWith(github.ref, 'refs/tags/v')" in workflow
    assert "contents: write" in workflow


def test_v140_release_notes_document_scope_validation_and_limits():
    notes = (ROOT / "docs" / "releases" / "v1.4.0.md").read_text(encoding="utf-8")

    assert "浏览器接管" in notes
    assert "403" in notes
    assert "177" in notes
    assert "Chrome" in notes and "Firefox" in notes
    assert "DRM" in notes


def test_build_requirements_pin_pyinstaller():
    requirements = (ROOT / "requirements-build.txt").read_text(encoding="utf-8")

    assert "-r requirements-dev.txt" in requirements
    assert "pyinstaller==6.19.0" in requirements.lower()

    release_lock = (ROOT / "requirements-release.lock").read_text(encoding="utf-8")
    release_workflow = _workflow("release.yml")
    assert "--hash=sha256:" in release_lock
    assert "pyinstaller==6.19.0" in release_lock.lower()
    assert "python -m pip install --require-hashes -r requirements-release.lock" in release_workflow


def test_readme_documents_windows_release_assets():
    readme = (ROOT / "README.md").read_text(encoding="utf-8")

    assert "ciaooo55/hls-downloader/actions/workflows/ci.yml" in readme
    assert "HLSDownloader-v3.0.18-Windows-x64-Setup.exe" in readme
    assert "HLSDownloader-v3.0.18-Windows-x64-Portable.zip" in readme
    assert "m3u8-sniffer.user.js" not in readme
    assert "HLSDownloader-v3.0.18-Firefox-Unsigned.zip" in readme
    assert "HLSDownloader-v3.0.18-Firefox-Source.zip" in readme
    assert "SHA256SUMS.txt" not in readme
    assert "Windows 10/11" in readme
    assert "git tag v" in readme
    assert "插件没有改动时不要上传独立插件包" in readme
