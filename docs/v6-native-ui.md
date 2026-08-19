# v6 native UI

The desktop product is a Windows-native Rust application. Slint renders every
visible window. `native_shell` owns `PersistentCore`, protocol workers, the
local media server, and Core IPC.

```text
Slint workbench / confirm / progress / complete / settings / player
        │ CorePipeRequest (length-prefixed JSON)
        ▼
CoreServer ── PersistentCore ── SQLite v6
        ▲
Browser Native Messaging host (stdio) ── same Core IPC, never opens the DB
```

```powershell
cargo test --manifest-path native_shell/Cargo.toml
cargo test --manifest-path native_ui/Cargo.toml
cargo run --manifest-path native_ui/Cargo.toml -- --self-test
```

The workbench restores tasks through Core IPC snapshots and `WaitEvents`.
Core events wake the Slint loop; clipboard watch is the only timer.
The process owns a Win32 tray icon (show / new task / settings / quit) and
hides the main window on close instead of exiting. Confirm, progress and
complete windows are created at boot and only `Show` on Core events.
`CoreServer::open_default` is the only SQLite open in this process: it owns
5.x migration and `HLS_V6_SKIP_LEGAL`. The Slint crate talks to Core only
through `CoreIpcClient::connect()` (named pipe on Windows). Native Messaging
is the same binary with `--native-host` and `HostCore::Remote` — the Local
SQLite backend is `cfg(test)` only.

Native Messaging `ping` speaks `hls-downloader-v6-core` only. It does not
return FastAPI `bridge_base` / `bridge_token`, so the extension must not
construct a loopback HTTP backend against the frozen 5.x server.

Do not call `PersistentCore::open` from `native_ui` or the Native Messaging
front-end. `hls-native-shell` is linked with `default-features = false` so
the 5.x Win32 supervisor and FastAPI client are not in `HLSDownloader.exe`.
Do not set `HLS_V6_CORE_BIND` in the product process; that re-enables
loopback TCP as a second IPC.

5.x extension manifests still point at the installed v5 host until installer
cutover. v6 registers `HLSDownloaderNativeHost.exe` as the same product binary
in Native Messaging mode (`--native-host` or executable name).
