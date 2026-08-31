# HLS Downloader 7 Rust Core

This crate is the only task, transfer, database and Native Messaging implementation in the active tree. Its default feature set is the v7 product; the removed Python/Win32 supervisor remains available through historical Git tags.

```powershell
cargo test --manifest-path native_shell/Cargo.toml --lib
cargo build --manifest-path native_shell/Cargo.toml --bin hls-downloader-engine
cargo build --manifest-path native_shell/Cargo.toml --bin HLSDownloaderNativeHost
```

The Core owns SQLite and serves the Compose workbench, browser host and native presenter only over the v7 named-pipe contract. Legacy data migration is handled by the dedicated migration modules, not by accepting the v6 wire protocol.
