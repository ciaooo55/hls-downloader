# Native shell rewrite

This branch keeps the current download core and browser extension. It replaces
the always-visible WebView2 desktop shell with a small resident native process
so the product can stay running like IDM: tray resident, instant confirmation,
small UI RAM, fast perceived startup.

Shipping `main` stays on Tauri until this shell is complete. Do not delete
HLS / DASH / live / BT / cast / takeover to get there.

## Why the current shell cannot feel like IDM

| IDM | Current Tauri shell |
| --- | --- |
| One native process already in the tray | Main window + extra WebView2 hosts for confirm / progress / complete |
| Click shows a window that already exists | Click may wait for a second WebView to boot and fetch |
| UI RAM is the dialog itself | Each WebView2 is a Chromium child |

The download engine is not the slow part. The click path is.

## Target process model

```
 login / first launch
        │
        ▼
 Native supervisor  ◄── always resident, tray only, pre-created hidden HWNDs
        │                 confirm + progress + complete
        │ named pipe / stdio (length-prefixed JSON)
        ▼
 Python core        ◄── FastAPI + HLS/DASH/BT/FFmpeg workers
        ▲                 listens on 127.0.0.1 only
        │
 Browser extension  ── Native Messaging ──► supervisor first, then core
```

- Closing the task list does **not** quit. Tray keeps the supervisor.
- The main list opens on demand. It is not required for takeover.
- Confirm / progress / complete windows are created once at boot, then shown
  and hidden. They are never a new WebView per click.
- Python core stays while the supervisor is resident, because live/HLS/BT
  workers already live there. Idle-exit of the core is a later option, not
  the first milestone.

## Footprint and speed (honest budget)

Matching IDM's 10–20 MB process is not possible while Python + libtorrent +
FFmpeg remain the engine. The rewrite still wins on the parts users feel:

- **Resident:** supervisor never unloads; no “start the app then click again”.
- **Small UI:** drop 3–4 WebView2 instances from the hot path. Dialogs are
  native surfaces, not Chromium.
- **Fast start:** login starts the tiny supervisor; the first click does not
  start a UI framework. Core warm is overlapped with tray boot.
- **Good result:** confirmation paints from the offer snapshot already on the
  pipe (`filename` / `url` / `size`). Settings and queue count can follow.

## What stays

Python FastAPI core, WXT MV3 extension, HLS / LL-HLS / DASH, live checkpoints,
BT/magnet, FTP/SFTP, Range HTTP, cast / TVBox, legal gate, loopback-only bind,
no DRM bypass, no IDM DLL / WFP / process injection.

## Phases

1. **Contract (this branch):** supervisor state machine + snapshot events + tests.
2. **Windows supervisor:** Rust tray + pre-created confirm/progress/complete.
   Extension Native Host talks to the supervisor pipe.
3. **Task list:** native main window still talking to the same `/api` as today.
4. **Cut over:** installer launches the supervisor instead of Tauri; Tauri
   remains until every window has a native twin.
5. **Optional later:** idle-stop the Python core when the queue is empty.

## Non-goals

- Kotlin/Compose revival, Electron, pywebview.
- Copying IDM capture DLLs or WFP/TDI.
- Rewriting HLS/BT in another language before the shell is resident.
