# HLS Downloader 7.0.1

Windows-first desktop download manager for resilient long-running transfers and native browser handoff.

Current v7 capabilities include:

- HTTP/HTTPS downloads with resume, mirrors, per-task request identity and recovery controls.
- HLS and DASH media downloads, including live/VOD workflows and authenticated request replay.
- FTP, SFTP and BitTorrent transfers alongside ordinary web downloads.
- Chromium and Firefox Manifest V3 browser integration with native confirmation, recovery and media push flows.
- Persistent Rust Core ownership of downloads and SQLite state, so closing the workbench, browser or player does not terminate active transfers.
- Windows desktop workbench, local playback, LAN casting/TVBox push, update/rollback validation and accessibility support.

The active product architecture is deliberately split by responsibility:

- `desktop_ui/`: Kotlin Compose Desktop shipping workbench.
- `native_shell/`: resident Rust Core, SQLite owner, transfer engines and Native Messaging host.
- `presenter_ui/`: small native presenter process for low-latency browser confirmation/progress/completion windows.
- `extension/`: WXT Manifest V3 extension for Chromium and Firefox.

Python, React, Tauri, WebView2 and the v6 Win32 supervisor are not part of the active source tree. Historical implementations remain available through Git tags, including `v3.0.39`, `v5.0.13` and `v6.0.1`.

## Architecture

`HLSDownloader.exe` never opens SQLite. It sends versioned commands to the single Rust Core over `\\.\pipe\HLSDownloader.v7`. The Native Messaging host and native presenter connect to the same Core. Closing Compose, the browser or the player does not stop active downloads.

The product version is `7.0.1`. `main` contains the complete active v7 source while historical implementations remain in Git tags. The existing `v7.0.0` release remains immutable; new release evidence and artifacts bind to `v7.0.1`.

## Build And Test

```powershell
# Rust Core
cargo test --manifest-path native_shell/Cargo.toml --lib

# Native hot presenter
cargo test --manifest-path presenter_ui/Cargo.toml
cargo build --manifest-path presenter_ui/Cargo.toml --bin hls-downloader-presenter

# Compose workbench
cd desktop_ui
.\gradlew.bat test --no-daemon

# Browser extension
cd ..\extension
pnpm install --frozen-lockfile
pnpm test
pnpm run build
```

Use `scripts\build-v7.ps1 -Task test` for the integrated local gate and `pwsh -NoProfile -Command "& { .\scripts\adversarial-v7.ps1 -Scope @('native','browser','transfer') }"` for the full fault/transfer matrix. `scripts\build-v7.ps1 -Task candidate` produces a machine-validation package under `artifacts\v7-productization\candidate`; it requires the canonical feature matrix, no blocked features and a clean Git worktree, while allowing partial features so candidate evidence can close them. It does not require `release_ready=true`. `scripts\build-v7.ps1 -Task package` produces the formal Windows App Image, EXE, MSI and Portable ZIP under `artifacts\v7-productization\package`; it requires all 28 features verified and adds the `release_ready=true` gate. `scripts\install-v7-local.ps1` performs an atomic per-user local upgrade to the single allowed install directory `E:\h`, retains the previous image as rollback, registers the v7 Native Messaging host, creates the Start menu shortcut, and republishes exactly one current Chromium/Firefox extension package each on the desktop, removing the previous copies.

Project build/tool caches default to `.tool-cache\build-cache`. Set `HLS_V7_BUILD_CACHE` to an **absolute** alternate cache root when the repository path or disk layout requires relocation; `bootstrap-v7-toolchain.ps1`, `build-v7.ps1` and `cleanup-v7-build-cache.ps1` all resolve and use that same root, and reject an ambiguous relative override. On Windows, the canonical build script temporarily maps the selected cache root to an ASCII drive path for Compose/jlink and removes the mapping on exit. Set `HLS_V7_JAVA_HOME` only to override the JDK 21 inside that cache, and `HLS_V7_PYTHON` for optional smoke tooling. Candidate and formal packaging require `ffmpeg.exe`, `ffprobe.exe` and `ffplay.exe` from one directory: set `HLS_V7_FFMPEG_DIR` explicitly, or place all three beside the `ffmpeg.exe` resolved from `PATH`. The ignored `desktop_ui\resources\common` staging directory is recreated for every package build and removed afterward so stale local binaries cannot leak into a later artifact.

Generated packages, test reports, runtime data and build caches are ignored by Git. `artifacts/v7-productization/feature-parity.json` is the sole machine-readable v3/v5/v6-to-v7 feature contract; validate it with `scripts\verify-v7-feature-parity.ps1`. See `docs/v7-verification.md` for measured results and remaining formal release gates.

## Source History

The repository is one history rather than multiple copied projects:

```powershell
git show v3.0.39:frontend/package.json
git show v5.0.13:backend/app/main.py
git show v6.0.1:native_ui/Cargo.toml
```

See `docs/source-layout-and-history.md`, `docs/v7-architecture.md`, `docs/v7-local-upgrade.md` and `docs/v7-verification.md`.
