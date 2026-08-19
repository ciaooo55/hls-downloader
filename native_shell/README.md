# Native supervisor (5.x, frozen)

This crate still builds `hls-native-shell` with the **`supervisor` feature**
(default): Win32 overlays, the FastAPI `CoreClient`, and spawning
`HLSDownloaderCore.exe`. That binary is the 5.x resident shell. Crash and
security fixes only.

**v6 product** is `native_ui`'s `HLSDownloader.exe`. It links this crate with
`default-features = false`, so the product process does **not** compile in the
Win32 task list, FastAPI loopback client, or Python-core spawners. Only
`CoreServer` opens SQLite. Slint and Native Messaging use
`\\.\pipe\HLSDownloader.v6`. Loopback TCP is opt-in for tests
(`HLS_V6_CORE_TCP` / `HLS_V6_CORE_BIND`).

5.x launch (frozen):

```
HLSNativeShell.exe --core-url http://127.0.0.1:8765/api --token <token>
```
