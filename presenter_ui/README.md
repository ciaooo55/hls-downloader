# HLS Downloader 7 native presenter

This crate contains the Windows-native hot-window compatibility surface. It uses Slint for the rendered desktop UI and consumes the shared Rust-side contract exported by `native_shell` through its `presenter-client` feature.

The presenter dependency intentionally uses `default-features = false`: it needs the v7 contract/IPC client plus launch, single-instance, profile-path, notification and native-window helpers, but it does not own or compile the resident `CoreRuntime` source path. Transfer execution and SQLite ownership remain in the default `full-core` product binaries.

```powershell
cargo test --manifest-path presenter_ui/Cargo.toml
cargo run --manifest-path presenter_ui/Cargo.toml --bin hls-downloader-presenter -- --self-test
```

The presenter never opens SQLite and never shows a second main workbench. It connects to the resident Rust Core, starting `HLSDownloaderEngine.exe` only when necessary. Confirm, progress and completion windows are created hidden at boot and consume the same v7 event stream as Compose.
