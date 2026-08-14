# Native supervisor (not shipping yet)

The resident Windows process will live here. Python `backend.app.native_shell`
is the contract and tests; this directory is the future Rust tray + pre-created
Win32/Slint windows.

Do not start a second WebView2 host from this process. The hot path is:

1. Supervisor already running in the tray.
2. Hidden confirm / progress / complete windows already created.
3. Browser offer arrives on the named pipe with a paint snapshot.
4. Supervisor shows the existing window.

See `docs/native-shell-rewrite.md`.
