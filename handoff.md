---
repository: ciaooo55/hls-downloader
default_branch: main
product_version: 7.0.2
release_ready: false
last_updated: 2026-10-02
---

# Project handoff

## Current operator-approved integration workflow (2026-09-15)

- Keep only local `main` and remote `origin/main`.
- Develop, review and validate on local `main`. Preserve useful source changes and dependency updates before removing redundant branch refs.
- Remote synchronization is operator-controlled. Do not push or otherwise write remote branches unless the operator explicitly asks.
- Do not create PRs, additional branches or additional worktrees.
- Dependency version-update PRs are paused; security audit workflows remain active. Formal-release gates and authorization requirements remain unchanged. Historical branch coordination records remain in Git history.
- Keep product source, validation evidence and deliverables in their designated directories. Preserve `.workbuddy-ai/` project state; remove obsolete copies only after protecting unmerged work and obtaining cleanup confirmation.

## Current baseline

- The canonical product state is `artifacts/v7-productization/feature-parity.json`: 27 verified, 1 partial, 0 blocked, `release_ready=false`.
- The remaining partial feature is `browser.media_push_device_selection`; its source path is implemented, but final acceptance still requires the installed-package real-browser and real-LAN-device gate.
- The active implementation is only `desktop_ui/`, `native_shell/`, `presenter_ui/`, and `extension/`.
- `native_shell` is the sole resident Core and SQLite owner. Compose and Presenter communicate through `hls-downloader-v7-core` on `\\.\pipe\HLSDownloader.v7`.
- `main` is the operator-authorized integration baseline under the workflow above. Preserve review and validation before pushing; this is not formal-publication authorization.

## Repository guidance

- Treat the sole local `main` plus the canonical feature matrix as the source of truth. Review and integrate useful changes there; push only with explicit operator authorization.
- The former multi-worker coordination scaffolding (`docs/coordination/`, `docs/worker-logs/`, `docs/manager.md`, `docs/architecture/coordination-protocol.md`) was removed once the single-`main` workflow superseded it; its history remains in Git.
- Historical Python/FastAPI, React/Tauri, WebView2, and v6 implementations remain available through Git history and tags; do not restore them as active directories.
- Candidate CI success does not authorize formal publication. Formal packaging still requires complete canonical parity, a separately reviewed `release_ready=true`, exact-main evidence, visual/performance/installer/rollback gates, signing, and explicit operator authorization.

## Validation

Repository validation commands remain defined by `AGENTS.md`. Keep `release_ready=false` until the real installed-browser/LAN media-push gate is accepted and canonical metadata is separately reviewed.
