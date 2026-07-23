# Design

## Intent

Windows-first download manager. Design **serves the task**: add URLs, watch progress, play media, recover failures. Users should trust it like Linear/IDM — familiar density, not decorative dashboard chrome.

## Tokens

CSS variables in `frontend/src/cockpit-shell.css` + Tailwind mapping in `frontend/src/styles/app.css`.

- Surfaces: cool slate neutrals (`--bg`, `--surface`, `--surface-2`, `--surface-3`)
- Ink: high-contrast body (`--text`), secondary (`--muted`), tertiary (`--faint`)
- Accent: single blue primary for actions/selection
- Status: green done, amber pause/merge, red failed, purple remux/parse

## Typography

System stack only: Segoe UI Variable / Microsoft YaHei UI. No remote webfonts in package.

## Layout

- Overlay titlebar drag region (Tauri)
- Floating left rail + solid top toolbar + main task table
- Modals for create/settings/player (not the default navigation pattern)

## Stack alignment

See [docs/architecture.md](docs/architecture.md).
