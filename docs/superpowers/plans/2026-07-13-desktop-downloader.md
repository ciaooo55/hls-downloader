# Desktop HLS Downloader Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver a Windows desktop HLS download manager with the existing reliable backend, a classic table UI, direct/page URL recognition, configurable userscript installation/export, and a verified installer.

**Architecture:** pywebview 6.2.1 hosts the existing React UI in an Edge Chromium window while uvicorn runs on a managed background thread. TaskManager remains the task lifecycle owner; new focused services handle desktop activation, URL recognition, and userscript rendering/export.

**Tech Stack:** Python 3.12, FastAPI, pywebview 6.2.1, WebView2, React 19, TypeScript, Vitest, PyInstaller, NSIS, ffmpeg/ffprobe

---

## File Map

- `backend/desktop.py`: desktop startup, server thread, activation, close ordering, and native bridge.
- `backend/app/desktop_runtime.py`: renderer-neutral activation callback registry.
- `backend/app/url_recognition.py`: classify direct HLS and statically discover webpage candidates.
- `backend/app/userscript_service.py`: render configured script and atomically export it.
- `backend/app/api.py`, `schemas.py`, `main.py`: expose activation, recognition, script status, and rendered installation routes.
- `frontend/src/desktop.ts`: typed pywebview bridge detection and native calls.
- `frontend/src/types.ts`, `format.ts`: shared task types and display formatting.
- `frontend/src/components/DesktopToolbar.tsx`, `Sidebar.tsx`, `TaskTable.tsx`, `TaskDetailsModal.tsx`, `RecognizeDialog.tsx`, `UserscriptDialog.tsx`: focused desktop UI units.
- `frontend/src/App.tsx`, `styles.css`, `api.ts`: compose the desktop application and theme.
- `scripts/build_installer.ps1`, `installer/hls-downloader.nsi`: freeze pywebview, stage resources, and install the desktop build.

### Task 1: Desktop Runtime Controller

**Files:**
- Create: `backend/desktop.py`
- Create: `backend/app/desktop_runtime.py`
- Create: `tests/test_desktop_runtime.py`
- Modify: `backend/run_server.py`
- Modify: `backend/app/api.py`
- Modify: `backend/requirements.txt`

- [ ] **Step 1: Write failing controller tests**

```python
def test_close_confirmation_stops_server_before_destroy(fake_window, fake_server):
    controller = DesktopController(fake_window, fake_server)
    fake_window.confirm_result = True
    assert controller.on_closing() is False
    controller.wait_for_shutdown()
    assert fake_server.calls == ["stop", "join"]
    assert fake_window.calls[-1] == "destroy"

def test_activation_restores_and_focuses_existing_window(fake_window):
    controller = DesktopController(fake_window, FakeServer())
    controller.activate()
    assert fake_window.calls == ["restore", "show"]
```

- [ ] **Step 2: Run `python -m pytest tests/test_desktop_runtime.py -q` and confirm import/behavior failures.**
- [ ] **Step 3: Implement `ServerThread`, `DesktopController`, and callback registry.**

```python
class DesktopController:
    def on_closing(self) -> bool:
        if self._allow_close:
            return True
        if not self.window.create_confirmation_dialog("退出下载器", "停止当前任务并退出？"):
            return False
        threading.Thread(target=self._shutdown_then_destroy, daemon=True).start()
        return False

    def _shutdown_then_destroy(self) -> None:
        self.server.stop()
        self.server.join(timeout=20)
        self._allow_close = True
        self.window.destroy()
```

- [ ] **Step 4: Add authenticated `/api/app/activate`; second launch probes health, posts activation, and exits.**
- [ ] **Step 5: Make `run_server.py` call desktop entrypoint and pin `pywebview==6.2.1`.**
- [ ] **Step 6: Run focused tests, then full Python tests.**

### Task 2: URL Recognition Service

**Files:**
- Create: `backend/app/url_recognition.py`
- Create: `tests/test_url_recognition.py`
- Modify: `backend/app/schemas.py`
- Modify: `backend/app/api.py`

- [ ] **Step 1: Write failing tests using local `httpx.MockTransport`.**

```python
async def test_recognizes_extensionless_playlist():
    result = await recognize_url("https://media.test/play?id=1", client=playlist_client())
    assert result.kind == "hls"
    assert result.candidates[0].url == "https://media.test/play?id=1"

async def test_extracts_and_deduplicates_relative_page_candidates():
    result = await recognize_url("https://site.test/watch/1", client=html_client())
    assert [item.url for item in result.candidates] == ["https://site.test/hls/master.m3u8"]
```

- [ ] **Step 2: Verify focused tests fail because the service is absent.**
- [ ] **Step 3: Implement bounded recognition.**

```python
class RecognitionResult(BaseModel):
    kind: Literal["hls", "page", "none"]
    final_url: str
    candidates: list[HlsCandidate]
    message: str = ""

async def recognize_url(url: str, headers: dict[str, str], client=None) -> RecognitionResult:
    owned = client is None
    http = client or httpx.AsyncClient(follow_redirects=True, timeout=15)
    try:
        async with http.stream("GET", url, headers=headers) as response:
            response.raise_for_status()
            body = bytearray()
            async for chunk in response.aiter_bytes():
                body.extend(chunk)
                if len(body) > 4 * 1024 * 1024:
                    raise RecognitionError("页面超过 4 MiB 识别上限")
            final_url = str(response.url)
            text = bytes(body).decode(response.encoding or "utf-8", errors="replace")
            if text.lstrip().startswith("#EXTM3U"):
                return RecognitionResult(kind="hls", final_url=final_url, candidates=[HlsCandidate(url=final_url)])
            candidates = extract_html_candidates(text, final_url, limit=100)
            kind = "page" if candidates else "none"
            message = "" if candidates else "页面未发现静态 HLS，请使用油猴脚本嗅探"
            return RecognitionResult(kind=kind, final_url=final_url, candidates=candidates, message=message)
    finally:
        if owned:
            await http.aclose()
```

- [ ] **Step 4: Validate schemes, redirects, content size, candidate count, and unsupported content messages.**
- [ ] **Step 5: Add authenticated `POST /api/recognize` and response schemas.**
- [ ] **Step 6: Run focused and full Python tests.**

### Task 3: Configured Userscript Rendering And Export

**Files:**
- Create: `backend/app/userscript_service.py`
- Create: `tests/test_userscript_service.py`
- Modify: `backend/app/main.py`
- Modify: `backend/desktop.py`
- Modify: `userscript/m3u8-sniffer.user.js`

- [ ] **Step 1: Write failing render/export tests.**

```python
def test_render_injects_current_api_and_token():
    text = render_userscript(source, host="127.0.0.1", port=9000, token="abc")
    assert "http://127.0.0.1:9000/api" in text
    assert "const TOKEN = 'abc';" in text

def test_export_is_atomic_and_refuses_overwrite_without_confirmation(tmp_path):
    target = export_userscript(tmp_path, "first", overwrite=False)
    with pytest.raises(FileExistsError):
        export_userscript(tmp_path, "second", overwrite=False)
    assert target.read_text(encoding="utf-8") == "first"
```

- [ ] **Step 2: Verify tests fail for missing service.**
- [ ] **Step 3: Render exact API/token constants with JSON-safe values and `Cache-Control: no-store`.**
- [ ] **Step 4: Implement atomic temporary write plus `os.replace`.**
- [ ] **Step 5: Expose `DesktopBridge.export_userscript(overwrite=False)` using `webview.FileDialog.FOLDER`.**
- [ ] **Step 6: Change the install route to serve rendered content and preserve heartbeat behavior.**
- [ ] **Step 7: Run userscript, desktop, and full Python tests.**

### Task 4: Desktop UI Foundation And Theme

**Files:**
- Create: `frontend/src/types.ts`
- Create: `frontend/src/format.ts`
- Create: `frontend/src/desktop.ts`
- Create: `frontend/src/styles.css`
- Create: `frontend/src/theme.test.ts`
- Modify: `frontend/src/main.tsx`
- Modify: `frontend/src/App.tsx`
- Modify: `frontend/package.json`

- [ ] **Step 1: Add `lucide-react` and write failing theme persistence tests.**

```ts
it('uses system theme until the user chooses an override', () => {
  expect(resolveTheme(null, true)).toBe('dark')
  expect(resolveTheme('light', true)).toBe('light')
})
```

- [ ] **Step 2: Verify Vitest failure for missing theme helpers.**
- [ ] **Step 3: Implement typed theme helpers and apply `data-theme` to the root element.**
- [ ] **Step 4: Introduce CSS variables for both themes, stable desktop dimensions, focus states, and overflow behavior.**
- [ ] **Step 5: Add typed task/settings models and shared format functions; remove duplicate inline formatters.**
- [ ] **Step 6: Run frontend tests and `pnpm run build`.**

### Task 5: Classic Download Table

**Files:**
- Create: `frontend/src/components/DesktopToolbar.tsx`
- Create: `frontend/src/components/Sidebar.tsx`
- Create: `frontend/src/components/TaskTable.tsx`
- Create: `frontend/src/components/TaskDetailsModal.tsx`
- Create: `frontend/src/taskCommands.ts`
- Create: `frontend/src/taskCommands.test.ts`
- Modify: `frontend/src/App.tsx`

- [ ] **Step 1: Write failing command-state and table selection tests.**

```ts
it('enables pause only when every selected task is pausable', () => {
  expect(commandState([downloadingTask]).pause).toBe(true)
  expect(commandState([downloadingTask, doneTask]).pause).toBe(false)
})
```

- [ ] **Step 2: Verify tests fail for missing command model.**
- [ ] **Step 3: Build icon toolbar with tooltips and disabled states from `commandState`.**
- [ ] **Step 4: Build status sidebar and userscript detection indicator.**
- [ ] **Step 5: Replace card list with a sortable, multi-select table and stable columns.**
- [ ] **Step 6: Move logs and full task metadata into dialogs; preserve all actions and API conflict messages.**
- [ ] **Step 7: Add aggregate bottom status including disk space returned by `/api/test`.**
- [ ] **Step 8: Run Vitest and production build.**

### Task 6: Recognition And Userscript Dialogs

**Files:**
- Create: `frontend/src/components/RecognizeDialog.tsx`
- Create: `frontend/src/components/UserscriptDialog.tsx`
- Create: `frontend/src/recognition.test.ts`
- Modify: `frontend/src/api.ts`
- Modify: `frontend/src/App.tsx`

- [ ] **Step 1: Write failing reducer tests for HLS, one candidate, multiple candidates, and no candidate states.**
- [ ] **Step 2: Verify tests fail before implementation.**
- [ ] **Step 3: Add typed `recognizeUrl`, `userscriptStatus`, and desktop bridge API calls.**
- [ ] **Step 4: Implement paste-and-recognize dialog with bounded loading and candidate selection.**
- [ ] **Step 5: Implement userscript dialog with install, export, overwrite confirmation, and current heartbeat status.**
- [ ] **Step 6: Ensure direct task creation and batch add remain available.**
- [ ] **Step 7: Run frontend tests and build.**

### Task 7: Packaging And Runtime Diagnostics

**Files:**
- Modify: `scripts/build_installer.ps1`
- Modify: `installer/hls-downloader.nsi`
- Modify: `tests/test_packaging_paths.py`
- Modify: `README.md`

- [ ] **Step 1: Add failing packaging assertions for pywebview hidden imports, desktop entrypoint, userscript, and WebView2 bootstrapper checks.**
- [ ] **Step 2: Verify focused packaging test failure.**
- [ ] **Step 3: Update PyInstaller arguments for pywebview Edge Chromium resources and suppress the console in release builds.**
- [ ] **Step 4: Stage the frontend, userscript, ffmpeg, ffprobe, and installer runtime checks.**
- [ ] **Step 5: Update NSIS shortcuts, version, runtime prerequisite handling, and uninstall cleanup.**
- [ ] **Step 6: Document desktop operation, URL recognition limits, and script export.**
- [ ] **Step 7: Run full Python/frontend/build verification.**

### Task 8: Desktop And Installer Acceptance

**Files:**
- Create: `tests/e2e/desktop_smoke.ps1`
- Create: `tests/e2e/hls_fixture.py`
- Modify: `scripts/build_installer.ps1`

- [ ] **Step 1: Generate a local VOD HLS fixture with ffmpeg and serve it on loopback.**
- [ ] **Step 2: Build the release installer from a clean stage.**
- [ ] **Step 3: Install silently to `build/installer/smoke-install` after verifying the path boundary.**
- [ ] **Step 4: Launch and verify one visible WebView2 window, health, duplicate activation, and no external browser process spawned by the app.**
- [ ] **Step 5: Exercise direct download, progress, pause/resume/cancel, merge, output existence, and ffprobe stream/duration validation.**
- [ ] **Step 6: Exercise script export and validate generated API/token metadata.**
- [ ] **Step 7: Capture default/minimum window screenshots in both themes and inspect blank pixels, clipping, and overlap.**
- [ ] **Step 8: Confirm close, uninstall, stop the fixture, remove only verified smoke paths, and assert no HLSDownloader/ffmpeg process remains.**

## Plan Self-Review

- Every design requirement maps to a task: desktop shell (1), recognition (2/6), userscript (3/6), themes/table (4/5), diagnostics/packaging (7), and installed acceptance (8).
- API names and data ownership are consistent: TaskManager owns lifecycle, `recognize_url` owns discovery, and `userscript_service` owns rendered script output.
- No deferred behavior or placeholder requirements remain.
- Git commit steps are omitted because the workspace has no `.git` repository.
