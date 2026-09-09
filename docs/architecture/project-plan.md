# Current execution plan — v7.0.2

## Baseline

- Canonical state: `artifacts/v7-productization/feature-parity.json`.
- Product version: `7.0.2`.
- Feature state: 27 verified, 1 partial, 0 blocked.
- Release state: `release_ready=false`.
- Active architecture: Compose workbench, one resident Rust Core/SQLite owner, native hot Presenter, and WXT MV3 extension.
- Historical Python/FastAPI, React/Tauri, WebView2, and v6 supervisor implementations remain in Git history only.

## Completed source work

- The v7 architecture and product cutover are on `main`.
- The security, updater, loopback transport, replay-origin, package, and release-gate hardening represented by merged PRs is retained.
- Browser media-push readiness now has exact-main prerequisite checks, interactive-runner checks, candidate-bound Native Host provenance, and fail-closed release wiring.
- Torrent input handling bounds peer metadata and piece sizes, rejects inconsistent piece metadata, verifies exact web-seed length and SHA1 piece hashes, and retries the next web seed after a failed integrity check.

## Remaining work

1. Run the repository validation suite on the final consolidated source.
2. Complete the installed-package Edge/Firefox to real LAN receiver gate for `browser.media_push_device_selection`.
3. If that evidence is accepted, update only the canonical feature entry in a narrow reviewed change.
4. Decide `release_ready` separately; do not infer it from candidate CI or source completeness.
5. Only after an authorized readiness transition, freeze the exact final `main` SHA and repeat every formal prerequisite workflow and trusted release gate.

## Historical records

`docs/coordination/`, `docs/worker-logs/`, `docs/manger.log`, and older readiness audits preserve earlier AI execution and audit history. They are not the live project plan and must not override this file, `AGENTS.md`, GitHub state, or the canonical feature matrix.
