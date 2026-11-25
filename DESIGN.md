# HLS Downloader 10.1.0 design contract

## Product structure

The Rust + Slint presenter is the main workbench. Rust Core is the only task, transfer and database owner. WXT is the only browser integration. The same presenter process also owns the latency-sensitive confirmation, progress and completion windows, while secondary tool/settings dialogs are allocated lazily on demand.

## Workbench geometry

- Native Windows caption bar with seamless DWM dark/light caption theme, paired with a compact responsive top command bar and recessed `Ctrl+F` search input.
- Full-height queue/category sidebar (`vertical-stretch: 1`) with a compact bottom transfer telemetry card, virtualized task table with interactive draggable scrollbar (`ScrollCue`) and queue drag-insertion indicator, and a collapsible non-modal right-side Task Inspector (`Alt+I` docked at `>= 900px`, responsive overlay below `900px`).
- Responsive column priority keeps `速度` visible down to `650px` table width so live transfer speed remains visible while the right-side Task Inspector is open.
- Keyboard shortcuts: `Ctrl+F` (search), `Ctrl+,` (settings), `Alt+I` (toggle inspector), `Enter` (task details), and `Space` (pause/resume selected task).
- Minimum effective workspace `1024x600` at `100%`, `125%` and `150%` scaling.
- Long labels, translated text and bottom actions must remain visible without mid-character clipping.

## Visual system

"Fluent Graphite + Luminous Cobalt" (refreshed in 10.1.0), shared by the Slint workbench, the hot confirmation/progress/completion windows and the browser extension.

- Cool-slate / graphite surfaces (`rail`, `canvas`, `surface`, `dialog`, `surface-2`, `surface-3`) with crisp cobalt primary actions (light `#2454d1` / `#2e5bea`, dark `#73a7ff` / `#7aa2ff`); dark mode puts dark ink on the light accent for WCAG AA contrast.
- Status colours and interactive borders (`>= 3.0:1` non-text contrast, `>= 4.5:1` text contrast) are verified across both Light and Dark themes.
- Secondary OS-framed dialogs rely directly on the native Windows titlebar without duplicating a second inner title banner.
- Non-activating `ProgressWindow` anchors near the bottom-right work area when the main workbench is closed and stays suppressed while the main workbench is visibly open; `ConfirmWindow` adapts its visible copy (`保存已接收文件` / `保存文件`) for already-received browser `POST`/`Blob` payloads while keeping Win32 window and button geometry contracts intact.
- Tokens live in `native_ui/ui/theme.slint` (`Tokens`) and `extension/lib/theme.ts` (`THEME_TOKENS_CSS`); never hard-code colours in views.
- Vector icons, contextual tooltips, focus rings and reduced-motion support.
- Hover, press, selection and status transitions must not shift layout.

## Interaction contract

Task selection supports Ctrl, Shift, select-all, keyboard navigation, capability-filtered secondary-click menus and visual drag ordering. Every action maps to a structured Core command and observable success/error result. Internal protocol or implementation names never appear in product copy.

Local playback opens in a main-workbench overlay with controls inside that surface. Picture-in-picture reuses the playback session. Casting and TVBox controls remain separate.

## Release contract

Formal publication requires the canonical contract, source-bound evidence, real transfer, presenter latency, UI Automation, performance and clean-machine installation gates. Unsigned prereleases explicitly state their tested scope and exclusions; they do not imply all formal gates passed. Publication requires operator authorization. Historical UI and behavior references are read from Git tags, not copied source trees.
