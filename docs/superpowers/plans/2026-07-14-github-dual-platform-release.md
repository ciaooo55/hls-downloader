# GitHub Dual-Platform Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish a source-only public GitHub repository that automatically tests and packages HLS Downloader for Windows x64 and Ubuntu/Debian Linux x64.

**Architecture:** Separate immutable application resources from writable per-user data, select desktop behavior by operating system, and keep platform packaging in focused scripts. GitHub Actions runs cross-platform CI on every change and composes versioned Release assets from independently verified Windows and Linux build jobs.

**Tech Stack:** Python 3.12, FastAPI, pywebview, PyInstaller, React, TypeScript, pnpm, NSIS, Debian packages, PowerShell, Bash, GitHub Actions.

---

## File Map

- Create `.gitignore`: exclude binaries, release output, dependencies, runtime data, credentials, and caches.
- Create `LICENSE`: MIT license for the public repository.
- Create `backend/app/platform_runtime.py`: operating-system-specific resource, data, FFmpeg, GUI, and browse-root helpers.
- Modify `backend/app/config.py`: consume resource/data helpers while preserving relative configuration compatibility.
- Modify `backend/desktop.py`: select Edge Chromium on Windows and GTK on Linux; show portable startup errors.
- Modify `backend/app/api.py`: use platform-aware folder browsing and file opening.
- Modify `config.json`: use a platform-neutral FFmpeg setting.
- Modify `tests/test_api_and_config.py`: verify installed and portable data/resource behavior.
- Modify `tests/test_packaging_paths.py`: verify both platform package layouts.
- Modify `tests/test_desktop.py`: verify GUI selection and startup fallback behavior.
- Create `tests/test_platform_runtime.py`: focused platform helper tests.
- Modify `scripts/build_installer.ps1`: consume tools from PATH, build setup and portable Windows assets, and support clean CI runners.
- Create `scripts/prepare_ffmpeg.ps1`: download/extract Windows FFmpeg into ignored `bin/`.
- Create `scripts/build_linux.sh`: build, stage, smoke-test, and emit `.deb` and `.tar.gz` assets.
- Create `packaging/linux/hls-downloader.desktop`: Linux application menu entry.
- Create `packaging/linux/hls-downloader`: Linux launcher.
- Create `.github/workflows/ci.yml`: Windows/Linux Python tests and frontend verification.
- Create `.github/workflows/release.yml`: manual and tag-triggered multi-platform packaging and release publishing.
- Rewrite `README.md`: public user/developer documentation and workflow badges.

### Task 1: Repository Hygiene And Public Metadata

**Files:**
- Create: `.gitignore`
- Create: `LICENSE`
- Test: repository file listing

- [ ] **Step 1: Write the source-only ignore policy**

Include exact entries for `.env*`, `bin/`, `release/`, `build/`, `tools/`, `downloads/`, `backend/downloads/`, `backend/data.db*`, `backend/build/`, `backend/dist/`, `frontend/node_modules/`, `frontend/dist/`, `.webview*/`, `.pytest_cache/`, `__pycache__/`, `*.py[cod]`, and PyInstaller spec output.

- [ ] **Step 2: Add the MIT license**

Use copyright holder `ciaooo55` and year `2026`.

- [ ] **Step 3: Verify ignored artifacts**

Run: `git status --short --ignored`

Expected: source files are untracked or tracked; `bin`, `release`, caches, databases, and build directories appear with `!!` and are not staged.

- [ ] **Step 4: Commit**

Run:

```powershell
git add .gitignore LICENSE
git commit -m "chore: prepare source-only public repository"
```

### Task 2: Cross-Platform Resource And Data Paths

**Files:**
- Create: `backend/app/platform_runtime.py`
- Modify: `backend/app/config.py`
- Modify: `config.json`
- Create: `tests/test_platform_runtime.py`
- Modify: `tests/test_api_and_config.py`

- [ ] **Step 1: Write failing path tests**

Add tests which monkeypatch the frozen executable, OS name, home directory, and environment. Assert that installed Linux resources resolve beside the executable while writable data resolves below `~/.local/share/hls-downloader`; a `portable` marker keeps data beside the executable; Windows retains its writable installed layout; and `default_ffmpeg_path()` returns `bin/ffmpeg.exe` on Windows and `bin/ffmpeg` on Linux.

- [ ] **Step 2: Run focused tests and verify failure**

Run: `python -m pytest tests/test_platform_runtime.py tests/test_api_and_config.py -q`

Expected: FAIL because `platform_runtime` and the new path behavior do not exist.

- [ ] **Step 3: Implement focused helpers**

Create pure functions with injectable arguments where practical:

```python
def is_windows() -> bool:
    return os.name == "nt"


def resource_root() -> Path:
    override = os.getenv("HLS_RESOURCE_DIR")
    if override:
        return Path(override).expanduser().resolve()
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    return Path(__file__).resolve().parents[2]


def data_root() -> Path:
    override = os.getenv("HLS_DATA_DIR")
    if override:
        return Path(override).expanduser().resolve()
    resources = resource_root()
    if (resources / "portable").exists() or not getattr(sys, "frozen", False):
        return resources
    if is_windows():
        return resources
    xdg = os.getenv("XDG_DATA_HOME")
    base = Path(xdg).expanduser() if xdg else Path.home() / ".local" / "share"
    return (base / "hls-downloader").resolve()


def default_ffmpeg_path() -> str:
    return str(Path("bin") / ("ffmpeg.exe" if is_windows() else "ffmpeg"))


def gui_backend() -> str:
    return "edgechromium" if is_windows() else "gtk"


def initial_browse_root() -> Path | None:
    return None if is_windows() else Path.home()
```

Use `HLS_RESOURCE_DIR` and `HLS_DATA_DIR` overrides for package launchers. Treat a `portable` file beside a frozen executable as portable mode. Do not move frontend or userscript resources into the writable data directory.

- [ ] **Step 4: Integrate configuration paths**

Keep `PROJECT_ROOT` as the immutable resource root for compatibility, add `DATA_ROOT`, store `config.json` in `DATA_ROOT`, and resolve downloads relative to `DATA_ROOT` while resolving bundled FFmpeg relative to `PROJECT_ROOT`. A saved absolute user-selected FFmpeg path remains absolute.

- [ ] **Step 5: Run focused tests**

Run: `python -m pytest tests/test_platform_runtime.py tests/test_api_and_config.py -q`

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```powershell
git add backend/app/platform_runtime.py backend/app/config.py config.json tests/test_platform_runtime.py tests/test_api_and_config.py
git commit -m "feat: add cross-platform runtime paths"
```

### Task 3: Cross-Platform Desktop And File Operations

**Files:**
- Modify: `backend/desktop.py`
- Modify: `backend/app/api.py`
- Modify: `tests/test_desktop.py`
- Create: `tests/test_platform_file_operations.py`

- [ ] **Step 1: Write failing desktop and file-operation tests**

Assert that Windows starts pywebview with `edgechromium`, Linux starts it with `gtk`, Linux startup errors do not access `ctypes.windll`, empty Linux browsing starts at the user's home directory, and file reveal uses `explorer`, `xdg-open`, or the containing directory as appropriate.

- [ ] **Step 2: Run focused tests and verify failure**

Run: `python -m pytest tests/test_desktop.py tests/test_platform_file_operations.py -q`

Expected: FAIL on hard-coded Windows GUI and Explorer behavior.

- [ ] **Step 3: Implement platform selection**

Call `gui_backend()` from `desktop.main()`. Keep the Windows native error dialog and add a Linux `zenity --error` attempt followed by stderr. Make storage writable below `DATA_ROOT/.webview`.

- [ ] **Step 4: Implement portable browse/open helpers**

Move subprocess argument construction into testable helpers. Return Windows drives only on Windows. On Linux, return the user's home directory listing when no path is supplied and use `xdg-open` for files or directories.

- [ ] **Step 5: Run focused and full Python tests**

Run: `python -m pytest -q`

Expected: all tests pass.

- [ ] **Step 6: Commit**

Run:

```powershell
git add backend/desktop.py backend/app/api.py tests/test_desktop.py tests/test_platform_file_operations.py
git commit -m "feat: support Linux desktop runtime"
```

### Task 4: Reproducible Windows Packaging

**Files:**
- Create: `scripts/prepare_ffmpeg.ps1`
- Modify: `scripts/build_installer.ps1`
- Modify: `installer/hls-downloader.nsi`
- Modify: `tests/test_packaging_paths.py`

- [ ] **Step 1: Write failing Windows package-layout tests**

Assert that the build emits both `HLSDownloader-Windows-x64-Setup.exe` and `HLSDownloader-Windows-x64-Portable.zip`, places a `portable` marker in the ZIP staging directory, finds `makensis` from PATH before downloading tools, and never requires committed `bin` files before the preparation step.

- [ ] **Step 2: Run tests and verify failure**

Run: `python -m pytest tests/test_packaging_paths.py -q`

Expected: FAIL because the portable artifact and FFmpeg preparation script do not exist.

- [ ] **Step 3: Add FFmpeg preparation**

Download a pinned Windows x64 FFmpeg archive from an explicit upstream release, verify the archive checksum recorded in the script, and extract only `ffmpeg.exe` and `ffprobe.exe` into ignored `bin/`.

- [ ] **Step 4: Update the Windows build**

Use an installed `makensis` when available. Run FFmpeg preparation if binaries are absent. Keep the existing staged-app health smoke test. Produce the renamed setup executable and a compressed portable staging directory containing `portable`, frontend assets, userscript, configuration defaults, and FFmpeg tools.

- [ ] **Step 5: Run package tests and a local Windows build**

Run:

```powershell
python -m pytest tests/test_packaging_paths.py -q
powershell -ExecutionPolicy Bypass -File .\scripts\build_installer.ps1
```

Expected: both Windows release assets exist and the staged application health check passes.

- [ ] **Step 6: Commit**

Run:

```powershell
git add scripts/prepare_ffmpeg.ps1 scripts/build_installer.ps1 installer/hls-downloader.nsi tests/test_packaging_paths.py
git commit -m "build: produce Windows setup and portable releases"
```

### Task 5: Linux Debian And Portable Packaging

**Files:**
- Create: `scripts/build_linux.sh`
- Create: `packaging/linux/hls-downloader`
- Create: `packaging/linux/hls-downloader.desktop`
- Modify: `tests/test_packaging_paths.py`

- [ ] **Step 1: Write failing Linux package-layout tests**

Assert that the launcher exports immutable resources and writable user data, the desktop entry invokes `/usr/bin/hls-downloader`, Debian control metadata declares x86-64 and GTK/WebKit/FFmpeg dependencies, and the script names both required Linux artifacts.

- [ ] **Step 2: Run tests and verify failure**

Run: `python -m pytest tests/test_packaging_paths.py -q`

Expected: FAIL because Linux package files do not exist.

- [ ] **Step 3: Implement the Linux launcher and desktop entry**

The launcher sets `HLS_RESOURCE_DIR=/opt/hls-downloader`, `HLS_DATA_DIR=${XDG_DATA_HOME:-$HOME/.local/share}/hls-downloader`, and executes the packaged binary. The desktop entry uses application categories `Network;Utility;`.

- [ ] **Step 4: Implement Linux packaging**

Build the frontend and PyInstaller executable, stage resources, include Linux FFmpeg binaries in the portable archive, create Debian metadata, build with `dpkg-deb`, and produce deterministic artifact names. Start the staged binary under a virtual display, poll `/api/health`, then stop it and verify port 8765 is free.

- [ ] **Step 5: Run package-layout tests**

Run: `python -m pytest tests/test_packaging_paths.py -q`

Expected: PASS locally; the executable package build is executed on GitHub's Ubuntu runner.

- [ ] **Step 6: Commit**

Run:

```powershell
git add scripts/build_linux.sh packaging/linux tests/test_packaging_paths.py
git commit -m "build: add Linux Debian and portable packages"
```

### Task 6: GitHub CI And Release Workflows

**Files:**
- Create: `.github/workflows/ci.yml`
- Create: `.github/workflows/release.yml`
- Create: `tests/test_github_workflows.py`

- [ ] **Step 1: Write workflow contract tests**

Parse workflow YAML as text and assert push/pull-request CI triggers, Windows/Linux Python matrix entries, manual and `v*` release triggers, least-privilege permissions, expected artifact names, checksum generation, and tag-only GitHub Release publication.

- [ ] **Step 2: Run tests and verify failure**

Run: `python -m pytest tests/test_github_workflows.py -q`

Expected: FAIL because workflow files do not exist.

- [ ] **Step 3: Add CI workflow**

Use `actions/checkout`, `actions/setup-python`, and `pnpm/action-setup`; cache dependencies; run Python tests on Windows and Ubuntu; run `pnpm install --frozen-lockfile`, `pnpm test`, and `pnpm run build` once on Ubuntu.

- [ ] **Step 4: Add release workflow**

Give only the final publish job `contents: write`. Windows and Ubuntu jobs run tests before packaging and upload their assets. The final job checks the complete expected file list, writes SHA256 hashes, uploads the combined bundle for manual runs, and uses a GitHub Release action only for `refs/tags/v*`.

- [ ] **Step 5: Run workflow tests**

Run: `python -m pytest tests/test_github_workflows.py -q`

Expected: PASS.

- [ ] **Step 6: Commit**

Run:

```powershell
git add .github/workflows tests/test_github_workflows.py
git commit -m "ci: add cross-platform builds and releases"
```

### Task 7: Public README And Versioned Documentation

**Files:**
- Modify: `README.md`
- Create: `docs/releasing.md`
- Test: README link and artifact-name checks

- [ ] **Step 1: Rewrite README**

Document the downloader UI first, then Release downloads, platform requirements, userscript installation/export, supported HLS features, source-only repository policy, Windows/Linux development, local packaging, and troubleshooting. Add CI and Release workflow badges pointing to `ciaooo55/hls-downloader`.

- [ ] **Step 2: Add maintainer release instructions**

Document manual workflow verification and the exact commands:

```powershell
git tag v1.1.0
git push origin v1.1.0
```

Explain that tags create Releases and ordinary pushes never commit or publish binaries.

- [ ] **Step 3: Verify links and names**

Run repository tests which assert every documented artifact name matches the workflows and build scripts.

- [ ] **Step 4: Commit**

Run:

```powershell
git add README.md docs/releasing.md
git commit -m "docs: document dual-platform downloads and releases"
```

### Task 8: Full Verification, Public Repository, And First Release

**Files:**
- Modify only files required by verification failures.

- [ ] **Step 1: Run local verification**

Run:

```powershell
python -m pytest -q
cd frontend
pnpm install --frozen-lockfile
pnpm test
pnpm run build
```

Expected: all Python and frontend tests pass and the production build succeeds.

- [ ] **Step 2: Review the source-only commit set**

Run: `git status --short --ignored` and `git ls-files`

Expected: no token, local database, download, FFmpeg binary, built installer, dependency directory, or runtime profile is tracked.

- [ ] **Step 3: Commit remaining source**

Stage the reviewed source tree and commit it without forcing ignored files.

- [ ] **Step 4: Create and push the public repository**

Use the authenticated GitHub API to create `ciaooo55/hls-downloader` as public with Actions enabled, add `origin`, and push `main`. Never persist the personal token in Git configuration or the remote URL.

- [ ] **Step 5: Verify CI and run a manual release build**

Poll the GitHub Actions API until CI and the manually dispatched release workflow finish. Inspect failed job logs, implement and push fixes, and repeat until both platforms pass and the combined workflow artifact contains every expected file.

- [ ] **Step 6: Publish the first version**

Create and push tag `v1.1.0`. Poll until the tag workflow succeeds. Verify the public Release has all six named assets and that `SHA256SUMS.txt` covers every downloadable binary/archive/script.

- [ ] **Step 7: Final repository audit**

Use the GitHub API to verify repository visibility is public, default branch is `main`, Actions are passing, the Release is published, and no generated binaries are part of the Git tree.
