# Windows Desktop Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a Windows installer with real FFmpeg, notification-area behavior, reliable exit, discoverable uninstall, migration, and complete cleanup.

**Architecture:** Introduce a focused runtime-path module, keep desktop lifecycle coordination in `backend/desktop.py`, and let NSIS own installation cleanup. Native bridge methods expose only the uninstall command needed by the React settings view.

**Tech Stack:** Python, pywebview, pystray/Pillow, FastAPI, React/TypeScript, PyInstaller, NSIS, GitHub Actions.

---

### Task 1: Runtime paths and migration

**Files:** `backend/app/paths.py`, `backend/app/config.py`, `backend/app/database.py`, `tests/test_runtime_paths.py`

- [ ] Add failing tests for source, installed, and portable path selection.
- [ ] Add failing tests for one-time legacy config/database migration.
- [ ] Implement `RuntimePaths` and migrate config/database consumers.
- [ ] Run the focused and full Python suites.

### Task 2: Tray and reliable shutdown

**Files:** `backend/desktop.py`, `backend/app/desktop_runtime.py`, `backend/app/api.py`, `backend/requirements.txt`, `tests/test_desktop_runtime.py`

- [ ] Add failing tests for close-to-tray, restore, idempotent exit, and remote shutdown.
- [ ] Implement a small tray adapter with Open and Exit actions.
- [ ] Route window close, tray exit, API shutdown, and final cleanup through one controller.
- [ ] Run desktop and API lifecycle tests.

### Task 3: Visible uninstall entry

**Files:** `backend/desktop.py`, `frontend/src/desktop.ts`, `frontend/src/components/SettingsPanel.tsx`, `frontend/src/desktop.test.ts`, `frontend/src/styles.css`

- [ ] Add failing tests for installed/portable uninstall availability and native invocation.
- [ ] Add a native uninstall method that starts `Uninstall.exe` only in installed mode.
- [ ] Add an uninstall command to the Settings dialog with confirmation and errors.
- [ ] Run Vitest and TypeScript build.

### Task 4: Installer cleanup and packaging

**Files:** `installer/hls-downloader.nsi`, `scripts/build_installer.ps1`, `tests/test_packaging_paths.py`, `README.md`, `docs/releasing.md`

- [ ] Add failing source-contract tests for graceful shutdown, fallback termination, recursive cleanup, download preservation prompt, tray dependency collection, and FFmpeg validation.
- [ ] Update NSIS install/uninstall behavior and PyInstaller collection.
- [ ] Document tray, runtime data, uninstall, and release contents.
- [ ] Run all Python and frontend tests.

### Task 5: Package and release acceptance

**Files:** `.github/workflows/release.yml`, generated `release/*`

- [ ] Push source and confirm GitHub CI succeeds.
- [ ] Build a manual GitHub artifact and inspect compressed/uncompressed FFmpeg sizes.
- [ ] Install to a temporary directory and run FFmpeg/FFprobe version checks plus API health.
- [ ] Exercise shutdown and silent uninstall, then confirm process, port, install path, app-data path, shortcuts, and uninstall registry entry are absent.
- [ ] Publish `v1.1.1`, mark `v1.1.0` as superseded, and sync verified assets to local `release/`.
