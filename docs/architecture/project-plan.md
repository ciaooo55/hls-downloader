# HLS Downloader v7.0.2 execution plan

## Current baseline

- Active development branch: `网页版gpt`.
- `main` remains a read-only integration baseline unless the user explicitly asks otherwise.
- Baseline `80512b037affc354a7339346541b50b02bbdd147` already passed normal source validation, Windows candidate packaging, and prerequisite workflows.
- Canonical product status remains 27/28 verified with `browser.media_push_device_selection` partial and `release_ready=false`.

## Development rules

1. Fix demonstrated product failures before adding features.
2. Do not add speculative compatibility, alternate state authorities, or abstractions without a current caller.
3. Keep Rust Core as the sole owner of download state and SQLite.
4. Keep the v7 browser transport on Native Messaging only; the retired FastAPI/v6 loopback backend is disabled and must not regain active routing.
5. Keep browser fallback ownership until the desktop transfer proves progress.
6. Keep changes small enough to validate independently.
7. Do not change canonical release readiness without the required real installed-browser/LAN receiver evidence.

## Priority order

### P0 — runtime correctness

Audit and harden task lifecycle, persistence, resume, cancellation, output publication, credentials, protocol boundaries, and browser handoff ownership. Only change behavior when a concrete failure is identified.

### P0 — browser media push source path

Keep the source-controlled path ready for the remaining real-device gate:

`extension -> Native Messaging -> Rust Core -> persisted media-push request -> Compose device picker -> LAN receiver -> browser status`

Do not manufacture receiver evidence or weaken the release gate.

### P1 — browser takeover reliability

Reduce false takeover, preserve exact request identity, keep short-lived URL replay safe, and make Chrome/Firefox ownership behavior consistent without adding legacy backends.

### P1 — protocol/download reliability

Prioritize real server/CDN failure modes: range inconsistency, redirects, expired authorization, HLS/DASH refresh, disk/output failures, and restart recovery.

### P1 — user-facing failure clarity

Keep internal state names and transport details out of the UI. Present actionable failure states without inventing recovery paths that the product cannot actually perform.

### P2 — structural cleanup

Split oversized modules only after behavior is stable. Structural commits must preserve behavior and must not simultaneously redesign protocol or lifecycle semantics.

## Release boundary

Formal v7.0.2 readiness still requires the protected Windows release environment and the existing installed-browser -> production Native Host -> Compose picker -> real LAN receiver gate. That external gate remains intentionally outside normal branch development.
