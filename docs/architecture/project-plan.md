# HLS Downloader v7.0.2 execution plan

## Current baseline

- Active development branch: `网页版gpt`.
- `main` remains a read-only integration baseline unless the user explicitly asks otherwise.
- Proxy route identity hardening landed in `e38bba5e7adb05404e36e3fa7f9cf75021354c6e` and passed focused Rust format/check/regression validation before commit.
- Windows POST body forwarding hardening landed in `9b6dae814d59d0b3a128084cc5dac18949c72b2d`; focused validation covered curl method/body arguments and a real WinHTTP POST to a local receiver.
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

### P0 — installed browser media-push gate

The source-controlled path is implemented and remains the only canonical route:

`extension -> Native Messaging -> Rust Core -> persisted media-push request -> Compose device picker -> LAN receiver -> browser status`

Existing automated evidence covers Native Host request IDs, Core persistence/resolution, Compose requested/resolved handling and completion feedback, and extension polling. The remaining acceptance step is external: run the installed candidate through real browser registration and a real LAN receiver. Do not manufacture receiver evidence, weaken the gate, or mark the feature verified from mocks alone.

### P0 — runtime correctness

Continue auditing task lifecycle, persistence, resume, cancellation, output publication, credentials, protocol boundaries, browser handoff ownership, request identity, and transport parity. Only change behavior when a concrete failure is identified.

Current reviewed code-level follow-up: duplicate-reuse request identity. Same-URL reuse must not silently retain stale method, credentials/replay context, headers, or request body. Any change should stay focused in `download_worker.rs` and ship with exact request-identity regressions.

### P1 — browser takeover reliability

Reduce false takeover, preserve exact request identity, keep short-lived URL replay safe, and make Chrome/Firefox ownership behavior consistent without adding legacy backends.

### P1 — protocol/download reliability

Prioritize real server/CDN failure modes: range inconsistency, redirects, expired authorization, HLS/DASH refresh, disk/output failures, restart recovery, direct/system/manual proxy correctness, and method/body parity across native and fallback transports.

### P1 — user-facing failure clarity

Keep internal state names and transport details out of the UI. Present actionable failure states without inventing recovery paths that the product cannot actually perform.

### P2 — structural cleanup

Split oversized modules only after behavior is stable. Structural commits must preserve behavior and must not simultaneously redesign protocol or lifecycle semantics.

## Release boundary

Formal v7.0.2 readiness still requires the protected Windows release environment and the existing installed-browser -> production Native Host -> Compose picker -> real LAN receiver gate. That external gate remains intentionally outside normal branch development. Candidate CI success alone is not publication authorization.
