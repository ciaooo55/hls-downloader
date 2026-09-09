# HLS-C021 — Fail fast on non-interactive media-push release runners

## Acceptance criteria

1. The pre-readiness media-push workflow must fail before toolchain/bootstrap/candidate build when the `hls-release` runner is not in an interactive Windows desktop session.
2. Session 0 service runners, `UserInteractive=false`, and current sessions without an Explorer desktop must be rejected with an actionable error.
3. `docs/v7-release-runner.md` must document the interactive desktop requirement, `HLS_V7_TVBOX_EXPECTED_HOST`, optional browser/driver overrides, clean MSI state and real LAN receiver requirements.
4. Preserve the existing fail-closed source/main/receiver/release-ready checks. No UI gate may silently skip because the runner is unsuitable.
5. Keep this task stacked on HLS-C020 until #84 merges; only then retarget this PR to `main` and revalidate the exact resulting diff.
6. HLS-C019 remains blocked until both C020 and C021 merge; the next readiness SHA is frozen only after both source fixes are on main.

## Finding

C019 requires headed Edge and Firefox, the installed Compose workbench, Java Access Bridge and a physical LAN receiver. The canonical release-runner document previously listed browsers and build/signing prerequisites but did not state that the GitHub runner agent must execute in a logged-in interactive desktop session. A runner installed as a normal Windows service can execute PowerShell/build work in Session 0 while being unable to expose the Compose/browser accessibility surface.

The same document also did not list the `v7-release` variable `HLS_V7_TVBOX_EXPECTED_HOST` that the readiness workflow already treats as mandatory.

## Implementation

Branch: `fix/hls-c021-interactive-runner-preflight` stacked on HLS-C020 frozen head `ed95b6613aa1af3dc6cf1d69192eb6ed2db9573e`.

- `scripts/assert-v7-interactive-runner.ps1`
  - requires Windows;
  - rejects Session 0;
  - requires `[Environment]::UserInteractive`;
  - requires an Explorer process in the current session;
  - emits a compact JSON success record with session/process identity.
- `.github/workflows/v7-media-push-readiness.yml`
  - invokes the assertion immediately after exact-main / receiver / `release_ready=false` checks;
  - therefore unsuitable runner environments fail before cache/toolchain/bootstrap/candidate work.
- `docs/v7-release-runner.md`
  - documents interactive desktop, receiver environment variable, clean install and optional driver overrides;
  - documents the pre-readiness workflow's authority boundary;
  - corrects formal release evidence wording to the current five candidate-bound gates: browser, browser_media_push, performance, installer and rollback.

## Safety

This task does not set `release_ready`, does not change feature verification status, and does not create/sign/publish release artifacts. It only makes the external validation environment explicit and fail-closed.
