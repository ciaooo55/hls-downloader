# HLS Downloader v6 architecture

v6 is a **single resident Rust process**. 5.x (Python FastAPI + Tauri/WebView2 +
Rust supervisor) is frozen: crash and security fixes only. New product work
lands in v6.

## Process model

```text
HLSDownloader.exe
  PersistentCore + SQLite v6     unique task truth
  named pipe \\.\pipe\HLSDownloader.v6
  WinHTTP Range + media/FTP/SFTP/BT workers
  Slint windows (pre-created confirm/progress/complete)
  local media server + in-process libmpv + LAN cast
        ▲
WXT MV3 extension  -- Native Messaging --  same Core (no SQLite in the host)
```

FFmpeg/ffprobe are spawned only for mux/verify. The browser extension stays
TypeScript because the browser requires it.

## Forbidden in the v6 process

- Python interpreter / FastAPI / uvicorn
- Tauri / WebView2
- yt-dlp as an in-process engine
- Opening the v6 SQLite database from the UI or Native Messaging process

UI and Native Messaging talk to Core through the versioned length-prefixed
JSON protocol (`hls-downloader-v6-core`) on `\\.\pipe\HLSDownloader.v6`.
Native Messaging `ping` never returns `bridge_base` / `bridge_token`; the
extension must not attach the 5.x FastAPI loopback when it sees that protocol.
Loopback TCP (`127.0.0.1:18765`) is opt-in (`HLS_V6_CORE_TCP` /
`HLS_V6_CORE_BIND`) for tests and non-Windows. Only `CoreServer` owns
`PersistentCore`. `native_ui` links `hls-native-shell` **without** the
`supervisor` feature, so `HLSDownloader.exe` does not contain the 5.x Win32
task list, FastAPI client, or `HLSDownloaderCore.exe` spawner. Those stay
behind `supervisor` for the frozen 5.x `hls-native-shell` binary.

## 5.x freeze

`backend/`, `frontend/`, and `frontend/src-tauri/` are a behavioral spec and
test mine. Do not add 5.x features. Do not call Python from v6.

Installer cutover happens only after the behavior matrix in
[v6-release-gates.md](v6-release-gates.md) passes. Packaging steps are in
[v6-cutover.md](v6-cutover.md).
