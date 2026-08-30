# HLS Downloader 7.0.0

Windows-first desktop download manager with one active product architecture:

- `desktop_ui/`: Kotlin Compose Desktop workbench.
- `native_shell/`: resident Rust Core, SQLite owner, transfer engines and Native Messaging host.
- `presenter_ui/`: small native presenter process for browser confirmation/progress/completion windows.
- `extension/`: WXT Manifest V3 extension for Chromium and Firefox.

Python, React, Tauri, WebView2 and the v6 Win32 supervisor are not part of the active source tree. Historical implementations remain available through Git tags, including `v3.0.39`, `v5.0.13` and `v6.0.1`.

## Architecture

`HLSDownloader.exe` never opens SQLite. It sends versioned commands to the single Rust Core over `\\.\pipe\HLSDownloader.v7`. The Native Messaging host and native presenter connect to the same Core. Closing Compose, the browser or the player does not stop active downloads.

The product version is `7.0.0`. `main` contains the complete active v7 source while historical implementations remain in Git tags. A final `v7.0.0` release tag is created only after the clean-machine release gates pass.

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

Build caches default inside the repository under `.tool-cache\build-cache`; set `HLS_V7_BUILD_CACHE` to relocate them, `HLS_V7_JAVA_HOME` for the JDK 21, and `HLS_V7_PYTHON`/`HLS_V7_FFMPEG_DIR` for the optional smoke and media tool locations.

Generated packages, test reports, runtime data and build caches are ignored by Git. `artifacts/v7-productization/feature-parity.json` is the sole machine-readable v3/v5/v6-to-v7 feature contract; validate it with `scripts\verify-v7-feature-parity.ps1`. See `docs/v7-verification.md` for measured results and remaining formal release gates.

## Source History

The repository is one history rather than multiple copied projects:

```powershell
git show v3.0.39:frontend/package.json
git show v5.0.13:backend/app/main.py
git show v6.0.1:native_ui/Cargo.toml
```

See `docs/source-layout-and-history.md`, `docs/v7-architecture.md`, `docs/v7-local-upgrade.md` and `docs/v7-verification.md`.
