# HLS Downloader 7 design contract

## Product structure

The Rust + Slint presenter is the main workbench (Compose remains a migration reference). Rust Core is the only task, transfer and database owner. WXT is the only browser integration. The same presenter process also owns the latency-sensitive confirmation, progress and completion windows.

## Workbench geometry

- Custom Windows title bar and 48dp command bar.
- Queue/category sidebar, virtualized task table and bottom status bar.
- One total progress indicator per task row; connection parts belong in details.
- Minimum effective workspace 1024x600 at 100%, 125% and 150% scaling.
- Long labels, translated text and bottom actions must remain visible.

## Visual system

"Graphite + Cobalt" (7.0.7 UI refresh), shared by the Slint workbench, the hot
confirmation/progress/completion windows and the browser extension.

- Neutral graphite surfaces with a single cobalt action colour
  (light `#2e5bea` / dark `#7aa2ff`); dark mode puts dark ink on the light accent.
- Status colours are theme-specific and keep at least 4.5:1 on their tinted pills.
- One app shell: full-height sidebar (brand, status filters, queues, categories),
  toolbar and page heading on the canvas, the task table as a single elevated card,
  and a slim status bar with engine/browser indicators.
- Task rows: file-type tile, two-line name/metadata, left-anchored progress bar,
  status pill with a colour dot, ghost row actions and a drag handle.
- Radii: two steps matching Windows 11 Fluent controls — controls (buttons, inputs, choice chips, row and nav highlights) 4; cards, banners, icon tiles, menus and frameless windows 8 (DWM rounded corners requested on Windows 11). Status pills, count badges and progress tracks are fully rounded. The software renderer does not clip children to a rounded parent, so any child that touches a rounded edge carries its own corner radius. Shadows only on
  primary buttons, the table card, menus and popups.
- Tokens live in `native_ui/ui/theme.slint` (`Tokens`) and
  `extension/lib/theme.ts` (`THEME_TOKENS_CSS`); never hard-code colours in views.
- Vector icons, tooltips, focus rings and reduced-motion support.
- Hover, press, selection and status transitions must not shift layout.

## Interaction contract

Task selection supports Ctrl, Shift, select-all, keyboard navigation, secondary-click menus and drag ordering. Every action must map to a structured Core command and observable success/error result. Internal protocol or implementation names never appear in product copy.

Local playback opens in a main-workbench overlay with controls inside that surface. Picture-in-picture reuses the playback session. Casting and TVBox controls remain separate.

## Release contract

Formal publication requires the canonical contract, source-bound evidence, real transfer, presenter latency, UI Automation, performance and clean-machine installation gates. Unsigned prereleases explicitly state their tested scope and exclusions; they do not imply all formal gates passed. Publication requires operator authorization. Historical UI and behavior references are read from Git tags, not copied source trees.
