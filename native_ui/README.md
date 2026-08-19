# HLS Downloader v6 native UI

This crate is the first Windows-native v6 workbench slice. It uses Slint for
the rendered desktop UI and consumes the shared Rust-side contract exported by
`native_shell`.

```powershell
cargo test --manifest-path native_ui/Cargo.toml
cargo run --manifest-path native_ui/Cargo.toml --bin HLSDownloader -- --self-test
```

The workbench never opens SQLite. It starts `CoreServer` in-process (or
connects to an already-running Core) and restores snapshots over Core IPC.
Search, category filters, counters, start/pause/delete, new-task and settings
are commands on that protocol. Confirm / progress / complete windows are
created hidden at boot.
