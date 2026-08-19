# v6 release gates

GitHub Windows Release ships v6. Keep this matrix green on a Windows
release build. 5.x trees remain a frozen behavioral spec.

## Build

1. `powershell -ExecutionPolicy Bypass -File scripts/run_v6_gates.ps1`
2. `cargo test --manifest-path native_shell/Cargo.toml --lib --no-default-features`
3. `cargo test --manifest-path native_ui/Cargo.toml`
4. `powershell -ExecutionPolicy Bypass -File scripts/build_v6.ps1 -Version 6.0.0-dev`
5. `powershell -ExecutionPolicy Bypass -File scripts/smoke_v6_package.ps1 -ArchivePath release/HLSDownloader-v6.0.0-dev-Windows-x64-Portable.zip`
6. Extension: `pnpm test` and `pnpm run build` in `extension/`

## Behavior matrix

- HTTP Range / no Range / interrupt / ETag change / POST single-stream
- Mirror failover with matching length/ETag
- Global and per-host connection budget; scheduled speed limit; WinHTTP origin connect reuse
- Site rules (host speed / concurrency / proxy / dir / UA / Referer) applied to new HTTP tasks
- Successful Range path: extra network bytes after publish = 0
- HLS TS concat, fMP4 local playlist, AES-128, LL-HLS PART, live stop
- DASH native representations; unsupported multi-period fails closed (no yt-dlp)
- FTP REST resume; FTPS (implicit 990 / AUTH TLS); SFTP TOFU fail-closed on host-key change
- Torrent/magnet import via `TorrentSession` (web seed; swarm frozen, not libtorrent)
- Optional post-complete AV scan (Defender or `{file}` template); skip when no scanner
- MOTW Zone.Identifier on public http(s) publishes
- GitHub latest-release check from Settings (does not auto-download)
- Browser Native Messaging offer/accept/reject without opening SQLite
- Playback Range server; cast URL is LAN-restricted; libmpv embed uses player HWND client area
- 5.x `config.json` + `data.db` migrate into v6 store and can be skipped

## Idle / click

- No WebView2, no Python in the resident process
- Confirm window is pre-created; first paint does not HTTP-fetch
- Click-to-confirm P95 target: under 100 ms when Core is already running

Setup.exe runs `register-native-host.ps1 -Cutover`.
