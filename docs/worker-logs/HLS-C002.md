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
- Product PR head at review start: `a99582c76e582210264ebc4e2f2761b6a41714f0`

## Independent source review

### Changed-file scope

PR #38 changes exactly four files:

- `.github/workflows/maintenance-security.yml`
- `.github/workflows/rust-security.yml`
- `scripts/invoke-v7-release-gates.ps1`
- `docs/v7-release-runner.md`

This is consistent with the claimed release-security integration scope.

### Main-push coverage

`Maintenance Security` removes the previous `push.paths` filter while retaining `pull_request.paths` for extension/workflow changes. `Rust Security` likewise removes its `push.paths` filter while retaining the Rust/workflow PR paths plus its scheduled/manual triggers.

Result: every `main` push can produce both security workflow conclusions, while pull requests remain path-filtered as intended.

### Exact candidate/source binding

The release-gate script reads the candidate manifest and computes current git commit/tree. Before any new security lookup it already rejects a candidate whose `source_commit` or `source_tree` differs from the checked-out source.

Inside `Assert-GitHubSecurityWorkflows`:

- the check executes only when `GITHUB_ACTIONS == true` and `GITHUB_WORKFLOW == 'v7 Formal Release'`, preserving non-formal/local/manual behavior;
- `GITHUB_REPOSITORY`, `GITHUB_SHA`, and `GH_TOKEN` are mandatory;
- `GITHUB_SHA.Trim()` must equal `$currentCommit` or the release fails closed;
- GitHub Actions runs are queried with `head_sha=$currentCommit`, `status=completed`, and up to 100 results;
- `Maintenance Security` and `Rust Security` are each accepted only when `name` matches, `event == 'push'`, `head_sha == $currentCommit`, and the newest matching run concludes `success`;
- missing, mismatched, or failed results throw and stop the formal release path.

Result: the source review satisfies the exact-SHA and GitHub workflow-context binding criterion.

### Documentation contract

`docs/v7-release-runner.md` is updated to describe the four required exact-SHA push runs (`v7 CI`, `v7 Candidate Package`, `Maintenance Security`, `Rust Security`) and explicitly treats a missing/failed security workflow as release-stopping.

## Workflow evidence for PR #38 head

For commit `a99582c76e582210264ebc4e2f2761b6a41714f0`, connector-reported PR-associated workflow runs were:

- `Maintenance Security` run #38 — `completed / success`
- `Rust Security` run #12 — `completed / success`
- `v7 CI` run #475 — `completed / success`
- `v7 Candidate Package` run #124 — `completed / success`

No failed required check was observed for the reviewed head.

## Auditor result

**PASS**, pending a final refresh of PR #38 mergeability/head immediately before merge. If the head moves, this audit must be repeated against the new SHA rather than reused.
