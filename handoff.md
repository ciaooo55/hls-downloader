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
- The remaining partial feature is `browser.media_push_device_selection`; its source path is implemented, but final acceptance still requires the installed-package real-browser and real-LAN-device gate.
- The active implementation is only `desktop_ui/`, `native_shell/`, `presenter_ui/`, and `extension/`.
- `native_shell` is the sole resident Core and SQLite owner. Compose and Presenter communicate through `hls-downloader-v7-core` on `\\.\pipe\HLSDownloader.v7`.
- `main` remains the integration baseline and must not be modified or merged from this workstream without explicit operator authorization.

## `网页版gpt` workstream

- This workstream is branch-only. All current hardening stays isolated on `网页版gpt` until explicit integration authorization.
- Recent hardening makes cURL long options with inline values (`--option=value`) follow the same validation path as split arguments, including proxy, cookie-file, user and data-urlencode rejection rules.
- The reviewed Metalink XML entity fix is integrated on this branch: predefined and numeric XML entities are decoded before remote-URL safety checks, preserving signed query strings such as `&amp;`.
- Proxy route identity hardening is complete in `e38bba5e7adb05404e36e3fa7f9cf75021354c6e`: direct/system/manual identities survive task/site/global policy resolution; site-rule `system` overrides global routes; bypass remains forced-direct; system routing uses WinHTTP automatic proxy selection; curl-impersonate does not silently bypass system routing; direct curl fallback disables environment proxying. It passed Rust format/check and focused route regressions before commit.
- HTTPS/custom POST body forwarding is complete in `9b6dae814d59d0b3a128084cc5dac18949c72b2d`: WinHTTP now sends the persisted request body instead of an empty request, and curl fallback preserves POST/HEAD method semantics plus the body without inventing a Content-Type. Windows CI exercised a real local WinHTTP POST receiver and the curl argument contract before commit.
- Duplicate reuse request identity hardening is complete in `c1d60cce6c7534130fd7021fff352a1a879e4963`: URL-based reuse is now limited to safe comparable requests and requires matching method and normalized non-sensitive headers; credential-backed, sensitive-header, POST/body requests do not reuse an older task. It passed Rust format/check and a focused identity regression before commit.
- Temporary repair workflows/scripts used to validate these fixes are removed after their tested source commits land; do not treat transient workflow commits as product surface.
- Next reviewed code-level hardening target is FTP/FTPS stalled data-channel pause/cancel responsiveness. Blocking data reads can delay control-file rechecks; any change must preserve Schannel behavior and include focused FTP/FTPS coverage rather than introducing naive short read timeouts.
- A later protocol-level follow-up remains Native Host transactional credential rollback: production IPC currently exposes Store/Load but not Delete, so proper rollback after a later task-creation failure requires a real protocol lifecycle change.
- The next product release gate remains `browser.media_push_device_selection`. Existing source verification covers Native Host request IDs, Core persistence/resolution, Compose requested/resolved handling, completion feedback, and extension polling. Do not mark it verified until the installed candidate is exercised through a real browser registration, Compose device picker, and a real LAN receiver.

## Repository guidance

- Treat `main` plus the canonical feature matrix as the release source of truth; branch-specific hardening remains isolated until explicitly authorized for integration.
- Files under `docs/coordination/`, `docs/worker-logs/`, and `docs/manger.log` are retained historical execution evidence, not live task-assignment instructions.
- Historical Python/FastAPI, React/Tauri, WebView2, and v6 implementations remain available through Git history and tags; do not restore them as active directories.
- Candidate CI success does not authorize formal publication. Formal packaging still requires complete canonical parity, a separately reviewed `release_ready=true`, exact-main evidence, visual/performance/installer/rollback gates, signing, and explicit operator authorization.

## Validation

Repository validation commands remain defined by `AGENTS.md`. Keep `release_ready=false` until the real installed-browser/LAN media-push gate is accepted and canonical metadata is separately reviewed.
