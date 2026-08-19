# v6 release gates

Cut over the installer only when every item below is green on a Windows
release build. 5.x stays the shipping line until then.

## Build

1. `powershell -ExecutionPolicy Bypass -File scripts/run_v6_gates.ps1`
2. `cargo test --manifest-path native_shell/Cargo.toml --all`
3. `cargo test --manifest-path native_ui/Cargo.toml`
4. `cargo build --manifest-path native_ui/Cargo.toml --release`
5. Extension: `pnpm test` and `pnpm run build` in `extension/`

## Behavior matrix

- HTTP Range / no Range / interrupt / ETag change / POST single-stream
- Mirror failover with matching length/ETag
- Global and per-host connection budget; scheduled speed limit
- Site rules (host speed / concurrency / proxy) applied to new HTTP tasks
- Successful Range path: extra network bytes after publish = 0
- HLS TS concat, fMP4 local playlist, AES-128, LL-HLS PART, live stop
- DASH native representations; unsupported multi-period fails closed (no yt-dlp)
- FTP REST resume; FTPS (implicit 990 / AUTH TLS); SFTP TOFU fail-closed on host-key change
- Torrent/magnet import, web seed / swarm pieces, and watch-folder
- Optional post-complete AV scan (Defender or `{file}` template); skip when no scanner
- GitHub latest-release check from Settings (does not auto-download)
- Browser Native Messaging offer/accept/reject without opening SQLite
- Playback Range server; cast URL is LAN-restricted
- 5.x `config.json` + `data.db` migrate into v6 store and can be skipped

## Idle / click

- No WebView2, no Python in the resident process
- Confirm window is pre-created; first paint does not HTTP-fetch
- Click-to-confirm P95 target: under 100 ms when Core is already running

Cutover is **not** automatic after these scripts exist. 5.x remains the shipping installer until this matrix is green on a Windows release build and `register-native-host.ps1 -Cutover` is run on purpose.
