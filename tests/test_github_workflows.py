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
    assert "pnpm test" in workflow
    assert "pnpm run build" in workflow
    assert "permissions:\n  contents: read" in workflow


def test_release_builds_only_windows_assets_and_publishes_tags():
    workflow = _workflow("release.yml")

    assert "workflow_dispatch:" in workflow
    assert 'default: "1.2.2"' in workflow
    assert "tags:" in workflow and "v*" in workflow
    assert "windows-latest" in workflow
    assert "ubuntu-latest" not in workflow
    assert "choco install ffmpeg nsis" in workflow
    assert "scripts\\build_installer.ps1" in workflow
    assert "HLSDownloader-Windows-x64-Setup.exe" in workflow
    assert "HLSDownloader-Windows-x64-Portable.zip" in workflow
    assert "m3u8-sniffer.user.js" in workflow
    assert "HLSDownloader-Firefox-Unsigned.zip" in workflow
    assert "HLSDownloader-Firefox-Source.zip" in workflow
    assert "HLSDownloader-Firefox-Signed.xpi" in workflow
    assert "web-ext sign" in workflow
    assert "SHA256SUMS.txt" in workflow
    assert "actions/upload-artifact@v7" in workflow
    assert "actions/download-artifact@v7" in workflow
    assert "softprops/action-gh-release" not in workflow
    assert "gh release create" in workflow
    assert '--repo "${{ github.repository }}"' in workflow
    assert "startsWith(github.ref, 'refs/tags/v')" in workflow
    assert "contents: write" in workflow


def test_build_requirements_pin_pyinstaller():
    requirements = (ROOT / "requirements-build.txt").read_text(encoding="utf-8")

    assert "-r requirements-dev.txt" in requirements
    assert "pyinstaller==6.19.0" in requirements.lower()


def test_readme_documents_windows_release_assets():
    readme = (ROOT / "README.md").read_text(encoding="utf-8")

    assert "ciaooo55/hls-downloader/actions/workflows/ci.yml" in readme
    assert "HLSDownloader-Windows-x64-Setup.exe" in readme
    assert "HLSDownloader-Windows-x64-Portable.zip" in readme
    assert "m3u8-sniffer.user.js" in readme
    assert "HLSDownloader-Firefox-Unsigned.zip" in readme
    assert "HLSDownloader-Firefox-Source.zip" in readme
    assert "SHA256SUMS.txt" in readme
    assert "Windows 10/11" in readme
    assert "git tag v1.2.2" in readme
