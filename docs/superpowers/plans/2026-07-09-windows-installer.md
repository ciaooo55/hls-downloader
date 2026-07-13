# Windows Installer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a standard Windows `.exe` installer for HLS Downloader that installs the packaged app to a chosen directory.

**Architecture:** Package the FastAPI backend as `HLSDownloader.exe` with PyInstaller. Keep mutable/runtime assets (`config.json`, `downloads`, `bin/ffmpeg.exe`, `frontend/dist`) outside the PyInstaller archive under the installation directory, then build a NSIS installer with shortcuts and uninstall support.

**Tech Stack:** Python, PyInstaller, React/Vite, NSIS.

---

### Task 1: Runtime Paths

**Files:**
- Modify: `backend/app/config.py`
- Modify: `backend/app/main.py`
- Create: `backend/run_server.py`
- Test: `tests/test_packaging_paths.py`

- [ ] Add a failing test that simulates `sys.frozen` and verifies `PROJECT_ROOT` resolves to the executable directory.
- [ ] Implement frozen-aware project root resolution in `backend/app/config.py`.
- [ ] Make `backend/app/main.py` use `PROJECT_ROOT / "frontend" / "dist"`.
- [ ] Add `backend/run_server.py` to launch uvicorn and open `/ui`.
- [ ] Run `python -m pytest tests/test_packaging_paths.py -q`.

### Task 2: Build Scripts

**Files:**
- Create: `installer/hls-downloader.nsi`
- Create: `scripts/build_installer.ps1`
- Modify: `README.md`

- [ ] Add a NSIS script that installs app files, writes uninstall metadata, creates Start Menu/Desktop shortcuts, and preserves existing user data on uninstall unless manually deleted.
- [ ] Add a PowerShell build script that builds frontend, builds PyInstaller output, stages files, downloads project-local NSIS if needed, and emits `release/HLSDownloaderSetup.exe`.
- [ ] Document the packaging command and output path.

### Task 3: Verification

- [ ] Run Python tests.
- [ ] Run frontend tests and build.
- [ ] Build the PyInstaller executable.
- [ ] Build the NSIS installer.
- [ ] Smoke test the packaged executable or installer output enough to verify `/api/health` responds.
