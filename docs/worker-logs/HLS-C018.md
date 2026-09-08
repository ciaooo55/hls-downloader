# HLS-C018 — installed-browser to LAN media-push release gate

## Acceptance criteria

1. Preserve canonical `release_ready=false` until the declared final-package media-push gap is actually closed.
2. Formal/release validation must fail closed on every residual canonical feature `gap`, even if a feature is accidentally labeled `verified`.
3. Add candidate-bound final-package validation using the candidate MSI / installed Native Messaging registration plus real Edge and Firefox production extension flow; execute the actual media-push request through Native Host/Core/device selection and prove delivery to a configured real LAN receiver. Missing external target/configuration must block formal release.
4. Bind all new release evidence to the exact source commit/tree and candidate manifest; predecessor or stale evidence is invalid.
5. `browser.media_push_device_selection` may remain `verified` only after reproducible release-bound evidence closes its gap. Until then it must be truthfully `partial` and canonical completeness/readiness must remain blocked.
6. Update release documentation and add regression coverage for the fail-closed contract.
7. Avoid a readiness/formal-release circular dependency: C018 must make the source/gate mergeable while truthfully partial, and a separate post-merge C019 workflow must be able to collect real-device evidence from exact current main while `release_ready=false`.
8. Use a dedicated PR and exact-head checks/review. If worker-0 remains unavailable, any fallback review is explicitly non-independent.

## 2026-09-09 — source finding

HLS-C014 v2 / PR #81 was closed unmerged after reverse release-gate audit found a contradiction in canonical metadata. `browser.media_push_device_selection` was labeled `verified` while the same record retained the gap `最终安装包仍需在真实浏览器注册和局域网设备上做实机门禁`.

The current release chain separated the relevant checks without proving the declared end-to-end path:

- real-browser smoke uses isolated extension profiles and deliberately does not touch installed Native Messaging registration;
- media smoke verifies 投屏/TVBox controls but does not execute the production `media_push` request;
- MSI lifecycle verifies Native Messaging registration exists/persists but does not drive a browser through that registration to a real LAN receiver;
- direct `smoke_v7_tvbox_real.py` proves a real receiver fetch, but starts from Core directly rather than the browser/Native Host production entrypoint;
- `verify-v7-feature-parity.ps1` did not reject a residual `gap` when release readiness was later enabled.

Therefore a formal run could satisfy the existing browser and MSI gates separately while the canonical final-package browser-to-LAN gap remained open.

## Implemented fail-closed layer

Commits on `fix/hls-c018-browser-media-push-release-gate` now add:

- `scripts/assert-v7-release-gaps.ps1`: rejects any non-empty canonical feature `gap` before formal release gates run;
- `scripts/test-v7-release-gap-contract.ps1`: behavioral regression proving no-gap input succeeds and residual-gap input fails;
- `scripts/invoke-v7-release-gates.ps1`: invokes the assertion immediately after exact candidate commit/tree validation and requires the new `browser_media_push` gate in addition to browser/performance/installer/rollback;
- `scripts/verify-v7-browser-media-push.ps1`: installs the exact candidate MSI into the fixed lifecycle path, verifies installed Edge/Firefox Native Messaging registration, starts installed Core/workbench, drives the production browser media-push request, uses packaged accessibility semantics to confirm device selection, requires the configured private-LAN receiver to fetch the deterministic media, and cleans the install afterward;
- `scripts/smoke_extension_tvbox_real.py`: drives Edge/Firefox production extension behavior and records receiver fetch plus browser executable/version/hash identity;
- `.github/workflows/ci.yml`: runs the gap-contract regression under Windows PowerShell 5.1 and PowerShell 7 and compiles the new Python smoke;
- canonical feature parity: `browser.media_push_device_selection` is reclassified `verified -> partial`, with summary 27/28 verified and `release_ready=false` unchanged.

This is defense-in-depth: feature status blocks canonical completeness and residual-gap assertion independently blocks formal release.

## Production path exercised

The gate tests the existing product path rather than replacing it with a test-only backend:

1. production extension TVBox action sends Native Messaging op `media_push`;
2. Core publishes `media_push_requested`;
3. Compose receives/restores the pending request and opens `DevicePickerDialog`;
4. the configured real receiver is discovered and preselected through the normal settings contract; direct Core `share_media` is not used to satisfy the gate;
5. packaged accessibility semantics activate `确认推送`, invoking the real `castToDevice` / `shareMedia` path;
6. the browser observes the resolved request and reports `已发送`;
7. the selected real LAN receiver itself must fetch the served deterministic media bytes.

The candidate MSI-installed Native Host registration is required; a temporary test-only registration cannot satisfy the gate.

## 2026-09-09 — circular dependency audit and C019 split

A second reverse audit found that running the real-device check only inside formal release would deadlock readiness:

- formal packaging requires `release_ready=true`;
- C014 must not grant readiness while the canonical feature remains `partial` with a release gap;
- therefore the first successful real-device gate must be executable before readiness is true.

C018 now adds `.github/workflows/v7-media-push-readiness.yml`. It is deliberately separate from formal release and has read-only repository permissions. After C018 merges, HLS-C019 will dispatch it from exact current `main` on the protected `hls-release` Windows runner while `release_ready=false`. It builds a candidate without formal-only readiness requirements, requires `HLS_V7_TVBOX_EXPECTED_HOST`, runs the same installed-browser real-LAN gate for Edge and Firefox, reconfirms main did not move, and uploads a candidate/source/tree-bound attestation.

The readiness attestation does not rely only on logs. It binds:

- exact source commit/tree and workflow run/attempt;
- candidate manifest digest and candidate MSI digest;
- installed Edge and Firefox Native Messaging registration snapshots;
- each browser report digest;
- browser executable digest/version identity;
- expected receiver identity and receiver-originated fixture fetch evidence.

Only a successful C019 run may authorize a later narrow metadata promotion that removes the gap and returns the feature to verified. That promotion must keep `release_ready=false`. C014 restarts afterward with fresh exact-main prerequisite checks.

The final formal release still reruns `browser_media_push` on the final readiness SHA; C019 evidence never substitutes for final-SHA release evidence.

## Validation checkpoint

At exact head `e0b082f762a293f6a9faeed0a8b9f96728a0ec2e`, v7 CI #551 started and its `Validate contracts` job completed successfully, covering Windows PowerShell 5.1/PowerShell 7 validation, the residual-gap regression, and Python compilation. Remaining Rust/Compose/browser/presenter jobs and v7 Candidate Package #200 must complete before C018 can leave draft/review state. Any head movement invalidates that checkpoint.

## Boundary

C018 may merge only as a source-hardening change with the feature still partial and `release_ready=false`. It does not tag, sign, dispatch formal release, create a GitHub Release or publish. C019 owns external readiness evidence; C014/C016 remain downstream governance/release stages.
