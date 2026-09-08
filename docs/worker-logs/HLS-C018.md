# HLS-C018 — installed-browser to LAN media-push release gate

## Acceptance criteria

1. Preserve canonical `release_ready=false` until the declared final-package media-push gap is actually closed.
2. Formal/release validation must fail closed on every residual canonical feature `gap`, even if a feature is accidentally labeled `verified`.
3. Add candidate-bound final-package validation using the candidate MSI / installed Native Messaging registration plus real Edge and Firefox production extension flow; execute the actual media-push request through Native Host/Core/device selection and prove delivery to a configured real LAN receiver. Missing external target/configuration must block formal release.
4. Bind all new release evidence to the exact source commit/tree and candidate manifest; predecessor or stale evidence is invalid.
5. `browser.media_push_device_selection` may remain `verified` only after reproducible release-bound evidence closes its gap. Until then it must be truthfully `partial` and canonical completeness/readiness must remain blocked.
6. Update release documentation and add regression coverage for the fail-closed contract.
7. Use a dedicated PR and exact-head checks/review. If worker-0 remains unavailable, any fallback review is explicitly non-independent.

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
- `scripts/invoke-v7-release-gates.ps1`: invokes the assertion immediately after exact candidate commit/tree validation and before browser/performance/MSI/rollback evidence is generated;
- `.github/workflows/ci.yml`: runs the gap-contract regression under Windows PowerShell 5.1 and PowerShell 7.

This is defense-in-depth. The canonical feature record is also being reclassified to `partial` until the real installed-browser/LAN evidence exists.

## Production path to exercise

The existing product path should be tested rather than replaced by a test-only backend:

1. production extension `pushToTv()` / `castToDevice()` sends Native Messaging op `media_push`;
2. Core publishes `media_push_requested`;
3. Compose restores/receives the pending request, discovers devices and opens `DevicePickerDialog`;
4. the preferred device can be selected through the existing settings contract;
5. activating `确认推送` calls the real `castToDevice`/`shareMedia` path and then resolves the pending media push;
6. the browser observes the resolved request and reports `已发送`;
7. a real LAN receiver must actually fetch the served media bytes, reusing the receiver-fetch proof model already present in `smoke_v7_tvbox_real.py`.

The final gate must use the candidate MSI-installed Native Host registration, not a temporary registration created only for the test.

## Boundary

No readiness, tag, signing, formal dispatch, release creation or publication is authorized by C018 until the gap is closed and subsequent C014/C016 stages pass.