# HLS-C022 — bind media-push readiness evidence to exact-SHA prerequisite workflows

## Finding

HLS-C019 requires `v7 CI`, `v7 Candidate Package`, `Maintenance Security`, and `Rust Security` to succeed for push/main/the exact prospective readiness SHA before real-device readiness evidence may be accepted. The pre-readiness workflow previously did not enforce that rule itself. A manual operator could dispatch `v7 Media Push Readiness` before those prerequisite runs completed and still create an otherwise well-bound readiness artifact, leaving a critical source-baseline condition only in out-of-band coordination state.

## Acceptance criteria

1. `.github/workflows/v7-media-push-readiness.yml` receives only the additional `actions: read` permission needed to inspect workflow runs.
2. After exact-current-main / configured receiver / `release_ready=false` validation, and before interactive-desktop/toolchain/candidate work, require successful completed runs for the exact `GITHUB_SHA` for:
   - `v7 CI` / `.github/workflows/ci.yml`
   - `v7 Candidate Package` / `.github/workflows/package-v7-candidate.yml`
   - `Maintenance Security` / `.github/workflows/maintenance-security.yml`
   - `Rust Security` / `.github/workflows/rust-security.yml`
3. Each accepted run must match canonical workflow name + path + `event=push` + `head_branch=main` + exact `head_sha`, and the latest matching run must conclude `success`.
4. Missing/failed/stale/predecessor-SHA runs fail closed before expensive real-device work.
5. No product/runtime feature behavior, canonical feature status, `release_ready`, signing, tag, GitHub Release, publication, or formal release behavior is weakened.
6. Exact-head `v7 CI` and `v7 Candidate Package` must pass before merge. After merge, HLS-C019 restarts again from the new exact main SHA; predecessor-SHA readiness evidence is invalid.

## Implementation

- Added `actions: read` while retaining `contents: read`.
- Added an early exact-SHA prerequisite step in `v7 Media Push Readiness`.
- The step uses the GitHub REST Actions API directly with `${{ github.token }}` and `Invoke-RestMethod`; it does not introduce a dependency on `gh.exe`.
- Matching semantics intentionally mirror the established formal-release gate: exact display name, canonical workflow path, push event, main branch, exact SHA, latest run number, success conclusion.
- The readiness workflow itself is `workflow_dispatch`, so it cannot satisfy any prerequisite because prerequisite matches require `event=push` and one of the four canonical workflow paths.

## Scope and safety

Implementation branch: `fix/hls-c022-readiness-exact-sha-prereqs`.
Base at claim: `30dd32a16719e2f3cd738f8bdafaffd550a2664b`.
First implementation commit: `722cb188deb2d2982961453f5de2c0698f4b4b1d`.

HLS-C019 is paused. Any successful prerequisite runs for `30dd32a...` become predecessor-SHA evidence if C022 merges and must not authorize the next readiness dispatch.

No readiness metadata promotion, tag, signing, formal release dispatch, Release creation, or publication is authorized by HLS-C022.
