---
repository: ciaooo55/hls-downloader
default_branch: main
product_version: 7.0.2
release_ready: false
last_updated: 2026-09-11
---

# Project handoff

## Current baseline

- The canonical product state is `artifacts/v7-productization/feature-parity.json`: 27 verified, 1 partial, 0 blocked, `release_ready=false`.
- The remaining partial feature is `browser.media_push_device_selection`; it still requires the installed-package real-browser and real-LAN-device gate.
- The active implementation is only `desktop_ui/`, `native_shell/`, `presenter_ui/`, and `extension/`.
- `native_shell` is the sole resident Core and SQLite owner. Compose and Presenter communicate through `hls-downloader-v7-core` on `\\.\pipe\HLSDownloader.v7`.
- The main source baseline includes bounded torrent metadata/piece allocation and piece-hash verification for HTTP web seeds.

## `网页版gpt` workstream

- This workstream is branch-only. Do not modify or merge `main` without explicit operator authorization.
- Recent hardening makes cURL long options with inline values (`--option=value`) follow the same validation path as split arguments, including proxy, cookie-file, user and data-urlencode rejection rules.
- The reviewed Metalink XML entity fix is integrated on this branch: predefined and numeric XML entities are decoded before remote-URL safety checks, preserving signed query strings such as `&amp;`.
- Highest-priority confirmed defect remains proxy route identity. Global settings expose `direct`, `system`, and `manual`, while site rules additionally expose inherit; current runtime collapses system/direct/bypass at `effective_proxy()` and WinHTTP treats the resulting empty value as `WINHTTP_ACCESS_TYPE_NO_PROXY`.
- Proxy acceptance criteria: preserve direct/system/manual identity end-to-end; site-rule `system` must override a global manual/direct route; bypass must remain forced-direct; WinHTTP and curl-impersonate fallback must agree on routing; add focused regressions for every route state before integration.
- Do not land a proxy sentinel in isolation. Every consumer of `Job.proxy` / effective proxy state must be updated in the same reviewed change.

## Repository guidance

- Treat `main` plus the canonical feature matrix as the current release source of truth; branch-specific hardening remains isolated until explicitly authorized for integration.
- Files under `docs/coordination/`, `docs/worker-logs/`, and `docs/manger.log` are retained historical execution evidence, not live task-assignment instructions.
- Historical Python/FastAPI, React/Tauri, WebView2, and v6 implementations remain available through Git history and tags; do not restore them as active directories.
- Candidate CI success does not authorize formal publication. Formal packaging still requires complete canonical parity, a separately reviewed `release_ready=true`, exact-main evidence, visual/performance/installer/rollback gates, signing, and explicit operator authorization.

## Validation

Run the repository validation commands from `AGENTS.md` only when active validation is authorized. Until the real browser/LAN media-push gate is accepted and canonical metadata is separately reviewed, keep `release_ready=false` and do not create a formal package or release.
