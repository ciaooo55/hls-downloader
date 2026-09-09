# HLS-C019 — candidate-bound installed-browser media-push readiness evidence

## Acceptance criteria

- Freeze the exact current `main` SHA after HLS-C020 and HLS-C021 are merged.
- Require `v7 CI`, `v7 Candidate Package`, `Maintenance Security`, and `Rust Security` to succeed for push/main/exact frozen SHA before an external readiness dispatch is accepted.
- Run `v7 Media Push Readiness` only on the `hls-release` interactive Windows x64 runner in the `v7-release` environment with a configured private IPv4 `HLS_V7_TVBOX_EXPECTED_HOST`.
- Require real Edge and Firefox using candidate extension identities and installed Native Messaging registration, production browser TVBox action, Compose DevicePicker selection and confirmation, and an actual media GET from the configured LAN receiver.
- Require the readiness workflow to finish on the same unchanged main SHA and upload candidate/source/tree/report-digest-bound readiness attestation.
- Do not promote `browser.media_push_device_selection`, clear its gap, change `release_ready`, tag, sign, create a Release, or publish without a successful exact-SHA external run and subsequent narrow reviewed metadata task.

## Current freeze

- Previous C019 target `a50f5dffedf45c8575464b47ebb8fe7f23039d96`: **invalidated** by C020/C021 source fixes; no predecessor-SHA evidence may be reused.
- Current prospective target: `30dd32a16719e2f3cd738f8bdafaffd550a2664b`.
- Current tree: `69280e77bf17caa3fbe8ab0965cf4bca1950780d`.
- HLS-C020: PR #84 merged as `ab7f33bdbe3a9a52cb43be2fed57a3a15abe8918`.
- HLS-C021: PR #85 merged as `30dd32a16719e2f3cd738f8bdafaffd550a2664b`.

## Exact-main prerequisite evidence

As of restart:

- Maintenance Security #74: `success`.
- v7 CI #562: `success`.
- Rust Security #48: `success`.
- v7 Candidate Package #211: `in_progress`; package build is the remaining prerequisite.

No `v7 Media Push Readiness` `workflow_dispatch` run is accepted before all four are successful and `main` is reconfirmed unchanged.

## External boundary

This ChatGPT GitHub connection can inspect workflow runs/jobs/logs/artifacts and rerun an existing run, but it does not expose an action to create a new `workflow_dispatch`. Therefore absence of a dispatch is an external/manual blocker, not permission to fabricate evidence or bypass the real-device gate.

## Preflight findings carried forward

- Native Messaging registry locations queried by the gate match `native_shell/src/native_host_registration.rs` HKCU Edge and Firefox registrations.
- Chromium candidate identity is pinned by `CHROMIUM_PUBLIC_KEY` to `bbdfldcjnikaemnimalegbopgaknjhla`; Firefox candidate identity is pinned to `hls-downloader-store@ciaooo55.com`, matching the installed Native Host allowlists.
- C020 removes the stale post-start preferred-device mutation and uses Java Access Bridge to select the configured DevicePicker row, then invokes `确认推送` separately.
- C021 rejects Session 0, non-interactive sessions, and sessions without Explorer before candidate build in the readiness workflow.
- Successful smoke still requires `/stream.mp4` to be fetched by the exact configured LAN receiver host.

## Safety

This branch/any evidence PR is audit-only and MUST NOT be merged while `30dd32a16719e2f3cd738f8bdafaffd550a2664b` is serving as the prospective readiness SHA. Merging coordination evidence would move `main` and invalidate exact-main evidence.
