# HLS-C002 — Audit PR #38 release security gate integration

## Acceptance criteria

1. Compare the actual PR #38 diff against the stated four-workflow formal-release contract.
2. Verify exact-SHA workflow identity/SHA binding from source rather than trusting the PR description.
3. Inspect workflow conclusions for the current PR #38 head.
4. Record PASS or actionable rejection evidence.
5. Merge PR #38 only if the audit passes and the required checks are successful.

## Assignment

- Priority: `P0`
- Auditor: `worker-0`
- Coordinator accepting this auditor task: `worker-0` under the explicit triple-role fallback
- Audit branch: `audit/hls-c002-pr38`
- Product PR under review: #38
- Superseded heads: `a99582c76e582210264ebc4e2f2761b6a41714f0`, `556722c4397e905dfd352dd60bfa03956d453ebd`, `9cb3bba5631ae54b328e4967a1019e758f54039c`
- Current head under audit: `8b5161eaf5bddeab061f714e5e8a88fa1e42f8ec`

## Audit history

### Initial implementation

The first implementation put GitHub workflow-identity logic into a general release-gates script. A later revision moved the authorization boundary into `.github/workflows/release-v7.yml`, avoiding a workflow-name-sensitive script bypass and adding canonical workflow-path matching.

### Exact-SHA trigger deadlock found and rejected

At head `9cb3bba...`, the formal workflow correctly required four exact-SHA identities, but `v7 CI` and `v7 Candidate Package` still had `push.paths` filters. The latest PR head only produced the two security checks, so prior-head CI/Candidate successes could not authorize it. HLS-C002 rejected that head and posted an actionable PR review.

The worker revised the branch in response instead of merging around the failure.

## Current-head independent source review

Current PR #38 changes exactly six release-governance files:

- `.github/workflows/ci.yml`
- `.github/workflows/maintenance-security.yml`
- `.github/workflows/package-v7-candidate.yml`
- `.github/workflows/release-v7.yml`
- `.github/workflows/rust-security.yml`
- `docs/v7-release-runner.md`

No generic/local release-gates script is modified.

### Four-workflow main-push contract

For all four prerequisite workflows (`v7 CI`, `v7 Candidate Package`, `Maintenance Security`, `Rust Security`), the `main` push trigger no longer has a paths filter. Their pull-request path filters remain in place.

Result: every `main` commit receives a conclusion from all four formal-release prerequisites, including docs-only or release-governance merges, while unrelated PRs do not automatically run heavyweight checks.

This resolves the pre-existing exact-SHA deadlock where the formal release insisted on current `main` but a path-filtered main commit could lack CI/Candidate results for that exact SHA.

### Formal workflow identity and SHA binding

Before building release inputs, `v7 Formal Release`:

1. requires dispatch from `refs/heads/main`;
2. requires checked-out HEAD equal current remote `main`;
3. queries completed Actions runs at exact `head_sha=$GITHUB_SHA`;
4. requires these canonical identities:
   - `v7 CI` / `.github/workflows/ci.yml`
   - `v7 Candidate Package` / `.github/workflows/package-v7-candidate.yml`
   - `Maintenance Security` / `.github/workflows/maintenance-security.yml`
   - `Rust Security` / `.github/workflows/rust-security.yml`;
5. requires exact name, exact workflow path, `event == push`, `head_branch == main`, exact `head_sha == GITHUB_SHA`, and `conclusion == success` for the newest matching run.

The job has `actions: read`, exposes `GH_TOKEN` from `github.token`, and fails closed on missing/mismatched/failed runs.

Result: duplicate-name, renamed-path, PR-event, wrong-branch, stale-SHA, missing, or failed runs cannot satisfy formal publishing.

### Local/manual behavior

The latest PR does not modify `scripts/invoke-v7-release-gates.ps1`. GitHub workflow identity policy therefore stays in the GitHub formal workflow rather than leaking into the generic/local gate script.

### Documentation

`docs/v7-release-runner.md` now states that all four prerequisites emit a push result for every `main` commit, explains canonical name/path + push/main/exact-SHA binding, and documents the fail-closed conditions.

## Current-head workflow evidence

For `8b5161eaf5bddeab061f714e5e8a88fa1e42f8ec`, all four expected PR checks are now present:

- `Maintenance Security` run #44 — completed / success
- `v7 CI` run #479 — in progress at last read; Browser extensions and Validate contracts jobs already succeeded, Rust Core/Compose/Presenter still running
- `v7 Candidate Package` run #127 — in progress at candidate build step
- `Rust Security` run #18 — in progress at pinned cargo-audit install/audit path

No current-head failure has been observed, but incomplete checks are not acceptance evidence.

## Auditor result

**SOURCE PASS / CHECKS PENDING.** The current source contract satisfies HLS-C002 criteria 1, 2, and 4, including resolution of the trigger deadlock identified by audit. Criteria 3 and 5 remain unsatisfied until the same head is refreshed and all four required checks conclude success. Any further head movement invalidates this current-head authorization state and requires another diff/check refresh.
