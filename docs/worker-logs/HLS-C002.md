# HLS-C002 — Audit PR #38 release security gate integration

## Acceptance criteria

1. Compare the actual PR #38 diff against the stated four-workflow formal-release contract.
2. Verify exact-SHA workflow lookup and `GITHUB_SHA` binding from source rather than trusting the PR description.
3. Inspect workflow conclusions for the current PR #38 head.
4. Record PASS or actionable rejection evidence.
5. Merge PR #38 only if the audit passes and the required checks are successful.

## Assignment

- Priority: `P0`
- Auditor: `worker-0`
- Coordinator accepting this auditor task: `worker-0` under the explicit triple-role fallback
- Audit branch: `audit/hls-c002-pr38`
- Product PR under review: #38
- Initial reviewed head: `a99582c76e582210264ebc4e2f2761b6a41714f0`
- Current head under re-audit: `556722c4397e905dfd352dd60bfa03956d453ebd`

## Initial head review — superseded

The initial head changed during the mandatory pre-merge refresh. The previous PASS for `a99582c...` is therefore **invalid for merge authorization** and is retained below only as history.

For initial commit `a99582c76e582210264ebc4e2f2761b6a41714f0`, all four observed PR-associated runs were completed/success, but that result must not be reused for the new head.

## Current-head independent source review

### Changed-file scope

Current PR #38 changes five files:

- `.github/workflows/maintenance-security.yml`
- `.github/workflows/release-v7.yml`
- `.github/workflows/rust-security.yml`
- `scripts/invoke-v7-release-gates.ps1`
- `docs/v7-release-runner.md`

The new file in scope relative to the initial review is `.github/workflows/release-v7.yml`.

### Main-push coverage

`Maintenance Security` removes the previous `push.paths` filter while retaining `pull_request.paths` for extension/workflow changes. `Rust Security` likewise removes its `push.paths` filter while retaining the Rust/workflow PR paths plus scheduled/manual triggers.

Result: every `main` push can produce both security workflow conclusions, while pull requests remain path-filtered as intended.

### Formal workflow exact identity gate

The updated `v7 Formal Release` workflow now queries completed runs for `head_sha=$GITHUB_SHA` and requires four explicit identities:

- `v7 CI` / `.github/workflows/ci.yml`
- `v7 Candidate Package` / `.github/workflows/package-v7-candidate.yml`
- `Maintenance Security` / `.github/workflows/maintenance-security.yml`
- `Rust Security` / `.github/workflows/rust-security.yml`

For each identity it additionally requires:

- exact workflow display name;
- exact canonical workflow path;
- `event == push`;
- `head_branch == main`;
- `head_sha == GITHUB_SHA`;
- newest matching run conclusion `success`.

The workflow has `actions: read`, sets `GH_TOKEN` from `github.token`, first refuses non-main/stale source, and performs this four-workflow gate before building the fresh candidate.

Result: a duplicate-name, wrong-path, PR-event, wrong-branch, wrong-SHA, missing, or failed workflow cannot silently satisfy the formal release prerequisite.

### Candidate/source binding in release-gate script

The release-gate script reads the candidate manifest and computes current git commit/tree. It rejects a candidate whose `source_commit` or `source_tree` differs from checked-out source.

Inside `Assert-GitHubSecurityWorkflows`:

- the extra lookup executes only when `GITHUB_ACTIONS == true` and `GITHUB_WORKFLOW == 'v7 Formal Release'`, preserving non-formal/local/manual behavior;
- `GITHUB_REPOSITORY`, `GITHUB_SHA`, and `GH_TOKEN` are mandatory;
- `GITHUB_SHA.Trim()` must equal `$currentCommit` or the release fails closed;
- the security lookup queries completed runs at exact `head_sha=$currentCommit` and requires matching push-event/head-SHA successful results.

The earlier formal-workflow identity gate is stricter because it also binds path and main branch. The later script check is redundant defense, not the sole authorization boundary.

### Documentation contract

`docs/v7-release-runner.md` now documents the exact four workflow names and canonical paths, plus `push`, `main`, and exact-SHA binding. It states that missing/renamed/path-mismatched/wrong-event/wrong-branch/wrong-SHA/failed required workflows stop release before candidate build.

## Current workflow evidence

For current head `556722c4397e905dfd352dd60bfa03956d453ebd`, the four required PR-associated checks were observed after the head update but are currently **in progress**:

- `Maintenance Security` run #39 — in progress
- `Rust Security` run #13 — in progress
- `v7 CI` run #476 — in progress
- `v7 Candidate Package` run #125 — in progress

A direct run read also confirmed the returned workflow `path` value uses canonical form such as `.github/workflows/ci.yml`, matching the new path comparison logic.

## Auditor result

**NOT YET ACCEPTED.** Source review of the current head is positive, but acceptance criterion 3/5 is not satisfied while the current-head checks remain in progress. PR #38 must not merge until the same head is refreshed and all four required checks conclude success. Any further head movement invalidates this current-head review again.
