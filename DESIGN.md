# Design

## Intent

Windows-first download manager. Design **serves the task**: add URLs, watch
progress, play media, recover failures. Users should trust it like Linear or
IDM — familiar density, not decorative dashboard chrome.

Product bar (v6):

- **Keep every capability** the 5.x spec already has (HTTP Range, HLS/LL-HLS/
  DASH/live, FTP/SFTP/BT, browser takeover, playback/cast, legal gate). Do not
  drop media to look like IDM.
- **Look and motion above AB Download Manager**: denser than Compose cards,
  52px task rows, 8-cell Range mosaic, hover row actions, 90–160ms state
  motion. No page-load choreography. Respect `Tokens.reduce-motion`.
- **Beat IDM on the hot path, honestly**: resident process, pre-created
  confirm window, WinHTTP Range with zero extra network bytes after publish,
  unknown-size probe, mirror identity, no WebView2/Python/JBR in the product
  process. Package size is allowed to exceed IDM because FFmpeg + libmpv ship
  for media. Idle working set is compared only when the player is not loaded.
  Do not claim a Windows release number until `docs/v6-release-gates.md` is
  green.

## Tokens

CSS variables in `frontend/src/cockpit-shell.css` (5.x frozen spec) and Slint
`Tokens` in `native_ui/ui/app.slint` (product). Same cool-slate ramp.

- Surfaces: `--bg` / `Tokens.bg`, `--surface`, `--surface-2`, `--surface-3`
- Ink: high-contrast body (`--text` / `Tokens.ink`), secondary (`muted`),
  tertiary (`faint`). Light muted is `#475569` so body text stays ≥4.5:1.
- Accent: single blue for actions/selection (`#2563eb` light, `#3b82f6` dark)
- Status: green done, amber pause/merge, red failed, purple torrent

## Typography

System stack only: Segoe UI Variable / Microsoft YaHei UI. No remote webfonts
in package. Product scale is tight (11 / 12 / 13 / 14), not display.

## Layout

- Overlay titlebar drag region (Slint workbench; 5.x used Tauri)
- Solid top toolbar + category rail + virtualized task table
- Confirm / progress / complete are pre-created; Show only
- Modals for create/settings/player (not the default navigation pattern)

## Motion

State only: hover 90ms, progress/selection 160ms, ease-out. Progress fill
width is the one layout property we animate. Hover row actions fade in place
so columns do not jump. No bounce, no elastic, no staggered page reveal.

## Stack alignment

v6 surfaces are Slint (`native_ui/ui/app.slint`). 5.x CSS in `frontend/` is
frozen. See [docs/v6-architecture.md](docs/v6-architecture.md).
