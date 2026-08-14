# Architecture

## Product stack

| Layer | Shipping on `main` | Target on `cursor/native-shell-rewrite-5a2e` |
| --- | --- | --- |
| Desktop shell | **Tauri 2** + WebView2 | Resident **native supervisor** (tray + pre-created dialogs). Task list on demand. See `docs/v5.0.0-plan.md`. |
| UI framework | **React 19** + **TypeScript** + **Vite 7** | Native confirm / progress / complete first; main list follows |
| Download core | **Python FastAPI** + uvicorn | Unchanged. HLS/DASH/HTTP/BT workers stay here |
| Browser extension | **WXT** (Chrome MV3 + Firefox) | Unchanged. Native Host talks to the supervisor pipe |
| Packaging | NSIS installer + portable zip + GitHub Actions | Installer starts the supervisor, not a WebView |

## Non-goals

- Do **not** reintroduce the removed Kotlin/Compose or pywebview desktop shells.
- Do **not** switch to Electron for the main window.
- Do **not** load remote Google Fonts in the packaged app (offline + privacy); use system UI fonts.
- Do **not** copy IDM capture DLLs, WFP/TDI, or process injection.
- Do **not** rewrite HLS/BT out of Python before the shell is resident, small, and instant.

5.0.0 is a major shell cut-over, not a new download engine. Full scope, IDM/ABDM takeaways, phases and acceptance numbers: `docs/v5.0.0-plan.md`. Stack / engine / how bytes become the final file: `docs/engine-stack.md`.

## File assembly

- **HTTP:** one `payload.downloading`, `Range` + `seek`, then rename. Never `cat` part files. NTFS sparse preallocate; one `r+b` handle per worker across capped 206s; Range payload writes off the event loop; checkpoint `fsync`s a dedicated handle. Sequential/no-Range writes each chunk immediately.
- **MPEG-TS HLS** (no init / discontinuity): FFmpeg `concatf:` local concat.
- **fMP4 HLS / DASH:** local `ENDLIST` playlist + FFmpeg timeline. Do not byte-concat `init.mp4` + `.m4s`.

## UI architecture

```
frontend/
  src/
    styles/app.css          # Tailwind theme + utilities
    styles.css              # Legacy dense component styles (player, tables, modals)
    cockpit-shell.css       # Floating workbench layout tokens
    lib/cn.ts               # className merge helper
    components/ui/          # Reusable primitives (Button, …)
    components/             # Feature panels
    store/uiStore.ts        # Shell UI state (filter/query/theme)
    App.tsx                 # Desktop manager composition root
    tauri.ts                # Desktop bridge
```

## Design language

- Register: **product tool** (Linear / IDM density, not marketing landing).
- Color strategy: restrained cool slate + single blue primary; semantic green/amber/red for task state.
- Motion: 150–200ms state feedback only; respect `prefers-reduced-motion`.
- Density: data-first tables, compact toolbar, floating side rail.

## Backend boundaries

- UI talks to local core over `http://127.0.0.1:<port>` with bearer token.
- Tauri owns windowing, tray, single-instance, folder dialogs, open-path.
- Core owns downloads, checksums, queue, speed throttle, native messaging, SSE progress.
