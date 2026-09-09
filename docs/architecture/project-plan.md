# Current execution plan — v7.0.2

## Baseline

- Working branch for this iteration: `网页版gpt`; `main` is read-only unless the user explicitly authorizes otherwise.
- Canonical state: `artifacts/v7-productization/feature-parity.json`.
- Product version: `7.0.2`.
- Feature state: 27 verified, 1 partial, 0 blocked.
- Release state: `release_ready=false`.
- Active architecture: Compose workbench, one resident Rust Core/SQLite owner, native hot Presenter, and WXT MV3 extension.
- Historical Python/FastAPI, React/Tauri, WebView2, and v6 supervisor implementations remain in Git history only.

## Verified baseline

The consolidated `80512b0` source baseline has already passed the ordinary Windows validation set: Rust Core, native Presenter, Compose tests and distributable image, browser extension tests/build, contract validation, Maintenance Security, Rust Security, and the Windows candidate-package workflow. Do not repeat those items as unfinished work and do not treat historical evidence as current after new source changes.

## Development rule

Only add code for current product behavior or a demonstrated failure mode. Do not add speculative compatibility layers, dormant alternate paths, future-proof abstractions without a caller, or defensive branches for situations the product contract does not support. Prefer deleting redundant state and keeping one owner/path over adding synchronization between multiple paths.

## Current priorities

1. Audit the live download/task lifecycle for concrete correctness failures: create/start/pause/resume/retry/complete/restart, HTTP range and redirect handling, HLS/DASH recovery, BT selection/materialization, and persistent state transitions.
2. Audit the browser takeover path for concrete ownership races and credential/replay mistakes. Keep Native Messaging + the resident Core as the production path; do not create another transport or state owner.
3. Audit the remaining `browser.media_push_device_selection` source path end-to-end: extension request -> installed Native Host contract -> persisted Core request -> Compose device picker -> LAN share/push -> browser resolution. Fix only source defects that can be demonstrated without fabricating real-device evidence.
4. Improve user-facing failure reporting where Core already has structured information but the workbench or extension loses it.
5. After behavior is stable, split oversized implementation files along existing responsibilities without changing protocol or behavior in the same commits.

## Release boundary

The only canonical partial feature is `browser.media_push_device_selection`. Its remaining gap is an installed-package, real Edge + Firefox, real Native Messaging registration, Compose device selection, and real LAN receiver gate. That evidence requires the protected release environment and must not be simulated or inferred from separate component tests.

Do not change the feature from `partial`, remove its `gap`, or set `release_ready=true` without accepted evidence for the exact source revision. Do not publish, tag, sign, or merge to `main` as part of ordinary branch development.

## Validation policy while developing on `网页版gpt`

- Run focused tests or CI appropriate to the changed module instead of repeatedly rebuilding every package after documentation-only or isolated source edits.
- For product-affecting changes, use the repository's existing Windows CI/PR checks as evidence; do not invent parallel validation machinery.
- PowerShell remains compatible with Windows PowerShell 5.1 and PowerShell 7.
- Protocol changes, if actually necessary, must update Rust contract and all live clients together; otherwise leave the protocol alone.
