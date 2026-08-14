# Native supervisor (5.0.0-alpha)

Python `backend.app.native_shell` is the contract, in-process supervisor, and
loopback IPC used by the download core today. This directory is the future
Rust tray process (Win32/Slint windows). Linux CI can only prove the contract
and IPC, not a real HWND.

## Hot path (wired on this branch)

1. Boot the supervisor (tests, `HLS_NATIVE_SHELL=1`, or
   `POST /api/desktop/native-shell/boot` with the desktop token).
2. Hidden confirm / progress / complete surfaces are marked warm.
3. Browser `POST /api/browser/handoffs` prefers the supervisor over Tauri.
4. The confirmation surface paints from the offer snapshot (`filename` /
   `url` / `size`). No extra HTTP is required for the first frame.
5. `GET /api/browser/presenter` reports `mode=native-shell` so Native Host
   does not wait on a Tauri window.

Closing the main list is `POST /api/desktop/native-shell/main/hide`. Tray
state stays resident.

Loopback IPC (length-prefixed JSON, same frames as the future named pipe):
`POST /api/desktop/native-shell/ipc/start`.

Do not start a WebView2 host from this process. Do not bump `APP_VERSION` to
5.0.0 until a real Windows supervisor binary shows those windows.
