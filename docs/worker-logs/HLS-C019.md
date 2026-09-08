# HLS-C019 — Candidate-bound installed-browser media-push readiness evidence

## Acceptance criteria

1. Freeze the exact current `main` SHA used by the readiness run; predecessor/future SHA evidence is invalid.
2. Keep canonical `release_ready=false` and `browser.media_push_device_selection=partial` during pre-readiness validation.
3. Run `.github/workflows/v7-media-push-readiness.yml` from exact current `main` on `[self-hosted, Windows, X64, hls-release]` in the `v7-release` environment.
4. Require configured `HLS_V7_TVBOX_EXPECTED_HOST`; missing receiver configuration or discovery is a hard failure, never a skip.
5. Require real Edge and Firefox through candidate MSI-installed Native Messaging registration -> Core -> Compose `DevicePickerDialog` -> real LAN receiver fetch of the deterministic media fixture.
6. Require source commit/tree, candidate manifest/MSI digest, installed Native Messaging registration snapshots, browser executable identities, per-browser report digests, and workflow run identity in `READINESS-ATTESTATION.json`.
7. Reconfirm remote `main` did not move during the run.
8. No tag, signing, formal release dispatch, GitHub Release creation or publication is authorized by this task.

## Frozen source

- main SHA at claim: `a50f5dffedf45c8575464b47ebb8fe7f23039d96`
- source merge: PR #82 (`release: close browser-to-LAN media-push gate`)
- canonical source state: 27/28 verified, 1 partial; `release_ready=false`

## Baseline push workflows

At the first C019 checkpoint for `a50f5dff...`:

- Maintenance Security #72: `success`
- v7 CI #553: `in_progress`
- v7 Candidate Package #202: `in_progress`
- Rust Security #46: `in_progress`

These baseline runs are useful source-health evidence but do **not** substitute for the C019 trusted-runner readiness workflow.

## Readiness workflow status

Querying workflow-dispatch runs for exact SHA `a50f5dff...` returned `total_count=0` at claim time.

The current GitHub connector exposes workflow reads and re-runs but does not expose creation of a new `workflow_dispatch`. Therefore this task is currently blocked at the trusted-runner dispatch boundary. No readiness PASS is claimed and no external evidence is fabricated.

## Evidence branch rule

This branch exists only to preserve C019 logs/evidence. It must **not** be merged into `main` while `a50f5dff...` is the frozen readiness candidate, because any merge would move `main` and invalidate exact-main readiness evidence.

## Next accepted evidence

When a `v7 Media Push Readiness` run appears for the frozen SHA, record:

- workflow run ID / attempt / conclusion;
- exact `head_sha` and event=`workflow_dispatch`;
- job/step conclusions;
- uploaded artifact ID/name;
- `READINESS-ATTESTATION.json` source commit/tree, candidate manifest/MSI digest, expected receiver, Edge/Firefox registration snapshots, browser identities, and report digests;
- post-run remote-main equality.

Only a complete same-SHA PASS may authorize the later narrow metadata-promotion task that removes the residual feature gap. C014 remains blocked until that promotion is independently reviewed.
