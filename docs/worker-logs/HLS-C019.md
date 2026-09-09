# HLS-C019 — candidate-bound installed-browser media-push readiness evidence

## Acceptance criteria

- Freeze the exact current `main` SHA after HLS-C020, HLS-C021, and HLS-C022 are merged.
- Require successful exact-SHA push/main runs for `v7 CI`, `v7 Candidate Package`, `Maintenance Security`, and `Rust Security`; the readiness workflow must independently enforce the same identity set before real-device work.
- Run `v7 Media Push Readiness` only on the `hls-release` interactive Windows x64 runner in environment `v7-release`, with configured private IPv4 `HLS_V7_TVBOX_EXPECTED_HOST`.
- Require real Edge and Firefox using candidate extension identities and installed Native Messaging registration, production browser TVBox action, Compose DevicePicker selection + confirmation, and an actual `/stream.mp4` GET from the configured LAN receiver.
- Require the readiness workflow to finish on the same unchanged main SHA and upload candidate/source/tree/report-digest-bound readiness attestation.
- Do not clear the feature gap, promote partial→verified, change `release_ready`, tag, sign, create a Release, or publish without a successful exact-SHA external run and a later narrow reviewed metadata task.

## Current frozen prospective target

- source commit: `178bf4276d9f2c8c8c485286a0ad66e5d59dd468`
- source tree: `4c3344becb42f635a4b9bf3049276bd467490ee3`
- predecessor targets `a50f5dff...` and `30dd32a...`: **invalid; never reuse**
- C020 / PR #84 merge: `ab7f33bdbe3a9a52cb43be2fed57a3a15abe8918`
- C021 / PR #85 merge: `30dd32a16719e2f3cd738f8bdafaffd550a2664b`
- C022 / PR #87 merge: `178bf4276d9f2c8c8c485286a0ad66e5d59dd468`

## Exact-main prerequisite evidence

At this evidence branch creation:

- Maintenance Security #75 / run `34304837652`: `success`.
- v7 CI #565 / run `34304837660`: `in_progress`.
- v7 Candidate Package #214 / run `34304837615`: `in_progress`.
- Rust Security #49 / run `34304837582`: `in_progress`.

No external readiness dispatch is authorized until all four are successful and `main` is reconfirmed identical to the frozen target.

## Machine-enforced readiness boundary

C022 added `scripts/assert-v7-exact-main-workflows.ps1` and wired it before interactive/toolchain/candidate/real-device work in `v7 Media Push Readiness`. The workflow now requires exact canonical workflow name + path + `event=push` + `head_branch=main` + exact SHA + success conclusion for all four prerequisite workflows. Missing, failed, stale, wrong-workflow, wrong-event, wrong-branch, or predecessor-SHA evidence fails closed.

C021 separately requires an interactive Windows desktop before expensive work. C020 separately requires explicit Compose DevicePicker selection through Java Access Bridge and a distinct confirmation action. Successful browser smoke still requires the exact configured receiver to fetch the deterministic media fixture.

## External boundary

The connected GitHub tool can inspect runs/jobs/logs/artifacts and rerun existing runs, but does not expose an action to create a brand-new `workflow_dispatch`. Runner inventory is also an unavailable administration endpoint. Therefore no trusted-runner availability or readiness PASS is claimed until a real `v7 Media Push Readiness` run appears for this exact SHA.

## Safety

This evidence branch/PR is audit-only and MUST NOT be merged while `178bf4276d9f2c8c8c485286a0ad66e5d59dd468` is the readiness candidate. Merging coordination evidence would move `main` and invalidate the exact-main prerequisite/readiness evidence.

Canonical must remain v7.0.2 / 27 verified / 1 partial / 0 blocked / `release_ready=false` throughout C019.
