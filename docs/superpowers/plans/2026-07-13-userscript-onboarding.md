# Userscript Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bundle the userscript, present an in-app tutorial, and detect recent userscript activity without claiming access to browser extension state.

**Architecture:** A focused in-memory monitor records userscript pings. FastAPI serves the script and a server-rendered help page, while the packaged entry point prints the same links. Existing PowerShell and NSIS packaging stages copy the userscript directory.

**Tech Stack:** Python 3, FastAPI, pytest, JavaScript userscript, PowerShell, PyInstaller, NSIS

---

### Task 1: Define observable behavior

**Files:**
- Create: `tests/test_userscript_support.py`
- Modify: `tests/test_packaging_paths.py`

- [ ] Add tests that request `/userscript/m3u8-sniffer.user.js` and `/help`.
- [ ] Add tests that POST `/api/userscript/ping` with `X-Token`, then GET `/api/userscript/status` and verify version, origin, and detection state.
- [ ] Add text-level packaging assertions for the staging and NSIS copy rules.
- [ ] Run `python -m pytest tests/test_userscript_support.py tests/test_packaging_paths.py -q` and confirm failures are caused by missing routes and copy rules.

### Task 2: Implement status monitoring and HTTP routes

**Files:**
- Create: `backend/app/userscript_monitor.py`
- Modify: `backend/app/api.py`
- Modify: `backend/app/main.py`

- [ ] Implement `UserscriptMonitor.record(version, page_url)`, `snapshot()`, and `reset()` with a 150-second freshness threshold and page-origin reduction.
- [ ] Add authenticated ping and status endpoints using the existing `_check_token` function.
- [ ] Serve the bundled `.user.js` file and a periodically refreshing Chinese `/help` page.
- [ ] Run the focused tests and confirm they pass.

### Task 3: Add userscript heartbeat and startup tutorial

**Files:**
- Modify: `userscript/m3u8-sniffer.user.js`
- Modify: `backend/run_server.py`

- [ ] Add a startup ping and 60-second heartbeat that silently ignores connection failures.
- [ ] Print the UI, tutorial, and script-installation URLs in the packaged console.
- [ ] Open `/help` at startup instead of opening `/ui` directly.
- [ ] Run the focused tests again.

### Task 4: Bundle and verify the installer

**Files:**
- Modify: `scripts/build_installer.ps1`
- Modify: `installer/hls-downloader.nsi`

- [ ] Copy the `userscript` directory into staging and install/uninstall it with NSIS.
- [ ] Run `python -m pytest -q` and the frontend test/build commands.
- [ ] Run `powershell -ExecutionPolicy Bypass -File .\scripts\build_installer.ps1`.
- [ ] Silently install into a temporary directory, launch the executable, verify health/help/script/ping/status, stop it, uninstall it, and confirm no process remains.

## Self-Review

The plan covers every design requirement, uses consistent route and field names, and contains no deferred implementation placeholders. This workspace is not a Git repository, so commit steps are intentionally omitted.
