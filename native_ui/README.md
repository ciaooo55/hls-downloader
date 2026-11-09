# HLS Downloader 7 native UI

This workspace member contains the Rust + Slint workbench and shared confirmation, progress and completion windows. `scripts/build-v7.ps1` defaults to native development and candidate packages; the Compose reference remains available with `-UiRuntime compose`. Formal publication remains blocked by parity and release gates. The migration is tracked in `docs/architecture/native-migration-plan.md`.

It depends on `hls-client` and `hls-protocol`, independently of the Core implementation and SQLite. Slint 1.17.1 uses explicit Windows winit, software rendering and accessibility features. Transfer execution and persistence remain in `native_shell`.

```powershell
cargo test --locked -p hls-native-ui
cargo run --locked -p hls-native-ui --bin hls-downloader-presenter -- --self-test
cargo run --locked -p hls-native-ui --bin hls-downloader-presenter -- --workbench
```

For the Cargo workbench command, first start `cargo run --locked -p hls-native-shell --bin hls-downloader-engine` in another terminal with the same profile and IPC environment. Cargo names the engine `hls-downloader-engine.exe`; automatic Core startup expects the installed layout with `HLSDownloaderEngine.exe` beside the UI. The self-test does not need a running Core. Cargo outputs alone are not a complete application package.

The UI never opens SQLite. It connects to the resident Rust Core, starting `HLSDownloaderEngine.exe` only when necessary. A per-profile activation pipe forwards subsequent workbench/presenter intents to the existing UI process.

The workbench uses paged snapshots and an independent event connection, stable task IDs and incremental row updates. It includes task creation, batch/cURL import, details, queue management, settings, resource tools and a media controller. Video uses the component HWND and physical client coordinates; main/PiP changes keep the same player session. Thumbnail work has a separate connection and a bounded queue. Service settings submit credentials directly to Core and never read stored plaintext back.

Toolbar start/pause applies to all tasks when nothing is selected, including tasks hidden by the current filter; a selection limits it to the selected rows. Batch start skips completed tasks. Use the explicit redownload context-menu action to download a completed task again.

Task columns fit the window automatically. The name fills the remaining width; speed and size columns return as space becomes available. Toolbar controls and search become compact in narrow windows. Appearance settings contain theme, motion and task sorting, without column-width, visibility or toolbar-arrangement editors. Native UI does not apply or rewrite the legacy layout preferences still used by Compose.

Restoring a saved window uses the current 560×360 logical minimum and clamps it to the monitor work area. Long queue headings elide within the available space and expose their full text in a tooltip, leaving the task actions available.

Completed ordinary files open with the file action; playback readiness only enables media actions for supported audio/video extensions or HLS/DASH/live tasks. Explicit Core playback capabilities remain authoritative. Opening a selected row's context menu preserves the current multi-selection.

Paste (`Ctrl+V`) accepts Explorer file selections, individual links, multiline batches and cURL text. Multi-file drops are submitted together for Core's partial-success reporting. Copy (`Ctrl+C`) writes the complete completed-file selection at once. Hold the context menu's drag-out item and drag to the destination; OLE runs on the UI thread while the mouse button is held. Windows file-transfer helpers are shared through `hls-client`, without adding Core or SQLite to the UI.

The cast window owns source/device selection and the visible session controls. Receiver discovery and commands use a separate bounded connection. Browser pushes recover their existing business IDs; unresolved transport failures do not automatically repeat a push. Local-player and cast controls carry independent session generations. Hover previews debounce mouse movement and reject frames from an old session or position.

`scripts/validation/smoke_native_workbench.py` exercises IPC recovery and simulated UI states. `smoke_native_media.py` starts an isolated real Core, authenticated local HTTP server, FFmpeg-generated media and libmpv. Neither script uses production task data. A passing simulated media fixture alone does not verify video rendering, browser handoff or release readiness.

`smoke_native_cast.py` uses a receiver bound to an address owned by this machine, a configured TVBox endpoint, real Core proxy Range reads, and optional native UI interaction. It does not validate multicast discovery or a physical receiver.

`smoke_native_tools.py --file-transfers` prepares real loopback downloads and two valid import lists with an invalid list between them. It checks Core's background clipboard ownership and the persisted tasks after GUI paste; it does not inject GUI input.

Online translation uses the existing Core subtitle service. Enable it from the player subtitle button after configuring the service key. Core decodes bounded windows from its own downloaded media over private loopback mounts; it never replays the origin request for translation. Main/PiP share captions, media-time offsets and source-language visibility. Seek, rate, track and session changes invalidate old results. The worker and network session are created on demand; this local implementation does not certify real cloud accuracy or end-to-end latency.

Native EXE/MSI/Portable candidates contain no Compose or JRE. `HLSDownloader.exe` is a short-lived native launcher for the shared presenter executable. Current parity and unverified items are recorded in `docs/validation/native-feature-mapping.md` and `docs/validation/native-migration-progress.md`; legacy source removal and formal publication still require complete release evidence.
