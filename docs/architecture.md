# Architecture

## Product stack (locked)

| Layer | Choice | Why |
| --- | --- | --- |
| Desktop shell | **Tauri 2** + WebView2 | Smallest popular native shell for Windows tools; not Electron |
| UI framework | **React 19** + **TypeScript** + **Vite 7** | Default popular SPA stack for desktop WebViews |
| Styling | **Tailwind CSS v4** + design tokens + existing component CSS | Industry default utilities; gradual migration without big-bang rewrite |
| Component variants | **cva** + **clsx** + **tailwind-merge** | shadcn/ui foundation pattern |
| Icons | **lucide-react** | Standard SVG icon set for product UI |
| Client state | **Zustand** | Lightweight store for shell UI state |
| Download core | **Python FastAPI** + uvicorn | Mature async API; HLS/DASH/HTTP/BT workers |
| Browser extension | **WXT** (Chrome MV3 + Firefox) | Popular extension toolchain |
| Packaging | NSIS installer + portable zip + GitHub Actions | Existing release path |

## Non-goals

- Do **not** reintroduce Kotlin/Compose as the primary shell (`desktop-compose/` is legacy).
- Do **not** switch to Electron for the main window.
- Do **not** load remote Google Fonts in the packaged app (offline + privacy); use system UI fonts.

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
