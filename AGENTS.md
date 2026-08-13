# AGENTS.md

## Cursor Cloud specific instructions

HLS Downloader is a **Windows-first** desktop download manager. The repo has three
buildable parts (see `README.md` "源码开发" for the canonical commands):

- `backend/` — Python 3.12 + FastAPI download core (also serves the web UI at `/ui`).
- `frontend/` — React 19 + Vite + Tauri 2 desktop shell / player.
- `extension/` — WXT + Manifest V3 Chromium/Firefox browser extension.

The cloud VM is **Linux (Ubuntu)**, but CI (`.github/workflows/ci.yml`) runs on
`windows-latest`. Everything below works on Linux except the Windows-only pieces
called out as expected caveats.

### Environment layout (already set up by the update script)

- Python deps live in a virtualenv at `.venv/` (gitignored). Activate with
  `source .venv/bin/activate`, or call tools directly via `.venv/bin/python`.
- `frontend/` and `extension/` use `pnpm@11.7.0` via corepack (`node_modules`
  are gitignored). Node 24 is used in CI; the VM's Node 22 also runs the Vitest
  suites and Vite builds fine.
- `ffmpeg`/`ffprobe` are at `/usr/bin` (only needed for HLS/DASH merging, not
  plain HTTP downloads).

### Running the app (dev)

1. Build the web UI once: `cd frontend && pnpm run build` (produces
   `frontend/dist`, which the backend serves at `/ui`). Backend returns a
   "Frontend not built" 404 at `/ui` until this exists.
2. Start the backend from the `backend/` directory:
   `python -m uvicorn app.main:app --host 127.0.0.1 --port 8765`
   Then open `http://127.0.0.1:8765/ui/`.
3. Optional frontend hot-reload dev server: `cd frontend && pnpm dev` (Vite on
   port 1420; the backend CORS allow-list includes `127.0.0.1:1420` only in
   non-frozen/dev mode).

Runtime state in source mode: `config.json`, `backend/data.db`, and `downloads/`
are created under the repo root and are all gitignored.

### First-run legal gate (non-obvious)

The backend refuses to create/resume/handoff downloads until the versioned legal
terms are accepted; acceptance is stored in the (gitignored) `config.json`. The
web UI shows a first-run "使用前请确认" dialog — tick the agree checkbox and click
"同意并继续" to unlock downloading. Deleting the `legal_terms_accepted_*` fields in
`config.json` and restarting the backend re-triggers the gate.

### Testing

- Backend: `python -m pytest -q`. On Linux, **8 tests fail and this is expected**
  — they are all Windows-only (DPAPI credential encryption, `WindowsPath`
  instantiation, `bin\\ffmpeg.exe` path-separator serialization, and
  frozen-executable/portable-upgrade packaging). ~726 tests pass. These are not
  regressions on Linux.
- Lint: `python -m ruff check backend tests` passes on Linux.
- Type-check: the CI mypy invocation reports 6 `ctypes.windll`/`WinError`
  `attr-defined` errors from `backend/app/credentials.py` **only on Linux**
  (those attributes exist only on Windows and are guarded at runtime by
  `os.name != "nt"`). Expected; not a code defect.
- Frontend: `cd frontend && pnpm test` (Vitest) and `pnpm run build`.
- Extension: `cd extension && pnpm test` and `pnpm run build`.

### Linux caveats

- Desktop-shell actions that shell out to Windows have no Linux equivalent, e.g.
  "open containing folder" surfaces a toast `目标系统不支持打开文件夹`. Downloads
  themselves still work; the toast is expected.
- `pnpm run tauri:build` (native desktop binary) needs Windows or a full Linux
  GTK/WebKitGTK toolchain and is **not** part of the web-served dev flow above —
  skip it unless specifically building the native shell.
