# HLS Downloader repository instructions

HLS Downloader is a Windows-first desktop download manager. The only active product version is `7.0.0`.

## Active architecture

- `desktop_ui/`: Compose Desktop main workbench. It never opens SQLite.
- `native_shell/`: single resident Rust Core, transfer engines, database owner and Native Messaging host.
- `presenter_ui/`: native hot presenter only; it is not a second main workbench.
- `extension/`: WXT Manifest V3 Chromium/Firefox extension.

The default protocol is `hls-downloader-v7-core` on `\\.\pipe\HLSDownloader.v7`. Legacy protocol handling exists only for explicit migration compatibility and must not become a default launch path.

Python/FastAPI, React/Tauri, WebView2 and the v6 Win32 supervisor are historical implementations. Their source is available through Git tags (`v3.0.39`, `v5.0.13`, `v6.0.1`) and must not be restored as active directories.

## Validation

```powershell
cargo test --manifest-path native_shell/Cargo.toml --lib
cargo test --manifest-path presenter_ui/Cargo.toml
cd desktop_ui; .\gradlew.bat test --no-daemon
cd ..\extension; pnpm test; pnpm run build
```

PowerShell scripts intended for users must parse under Windows PowerShell 5.1 and PowerShell 7. Text JSON/manifests must be written as UTF-8 without BOM unless the target format requires otherwise.

Do not generate formal packages until `artifacts/v7-productization/feature-parity.json` is fully verified and all visual, performance, installer and rollback gates pass.
