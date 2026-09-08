# HLS-C014 — Decide canonical v7.0.2 release readiness

## Acceptance criteria

1. Revalidate the accepted HLS-C013 source/CI evidence and canonical 28/28 completeness on the post-C015 governance baseline.
2. Make an explicit reviewed decision whether canonical `release_ready` may transition to `true`; if not, preserve `false` and record the concrete blocker.
3. If authorized, change readiness metadata narrowly and do not weaken formal package/release gates.
4. After the final reviewed merge, freeze the resulting `main` SHA and require fresh successful `v7 CI`, `v7 Candidate Package`, `Maintenance Security`, and `Rust Security` `push/main/exact-SHA` results before formal dispatch.
5. Never tag, sign, release, dispatch or publish without the trusted runner/signing/browser/environment controls and explicit operator authorization.

## Assignment

- Priority: P0
- Owner: `worker-0`
- Role: coordinator + auditor + worker fallback
- Branch: `coord/hls-c014-release-readiness-decision`
- Current base: `main@d37e8cac666c3ebd7f3c8ffa326a15321ef76185` after HLS-C015 / PR #78

## Evidence inherited and revalidated

HLS-C013 audited frozen `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea` and found:

- canonical v7.0.2 matrix = 28/28 verified, 0 partial, 0 blocked;
- exact-SHA main-push `v7 CI` #525, `v7 Candidate Package` #173, `Maintenance Security` #68, and `Rust Security` #42 all successful;
- no C009/C010/C011 product-security hardening overwritten;
- formal workflow and package verifier fail closed on exact-SHA provenance, current-main drift, canonical completeness, `release_ready`, clean worktree, release evidence, trusted runner/browser/signing controls, digest verification and explicit publish choice.

HLS-C015 then changed governance/documentation only. It corrected root `AGENTS.md` to active v7.0.2, bound live version/readiness truth to canonical feature-parity metadata, and made explicit that formal packaging requires fully verified parity plus a separately reviewed `release_ready=true` decision and the visual/performance/installer/rollback gates. It did not change product/runtime/workflow/build-script/feature-parity source.

The post-C015 main SHA `d37e8cac666c3ebd7f3c8ffa326a15321ef76185` created all four required main-push prerequisite workflows, demonstrating trigger coverage.

## Historical readiness semantics

The repository itself provides the state-transition precedent. At the v7.0.1 release-candidate state, canonical feature parity used:

- `release_ready = true`
- `audit_state = v7_0_1_release_specialties_verified`

The automated `build: start v7.0.2 iteration` commit then deliberately reset those two fields together to:

- `release_ready = false`
- `audit_state = v7_0_2_iteration_in_progress`

while incrementing the product version. No executable verifier treats `audit_state` as an external-release proof; the formal package gate checks `release_ready=true`, canonical completeness, clean worktree and commit/tree/candidate-bound release evidence.

Therefore C014 interprets `release_ready=true` as the reviewed repository-governance authorization to enter the existing formal validation/release pipeline. It does **not** mean the trusted Windows runner, browser evidence, MSI lifecycle/rollback evidence, signing certificate, timestamp service, protected environment, draft asset verification, or operator publication approval have already been satisfied.

## Decision condition

C014 may authorize the narrow transition only if all of the following remain true at decision time:

1. canonical v7.0.2 matrix is still 28/28 verified with no partial/blocked entries;
2. no new source/security/governance blocker has been reproduced since C013/C015;
3. post-C015 main has not moved unexpectedly and its four required main-push workflows have no failure conclusion;
4. the readiness change is limited to canonical readiness metadata plus coordination/audit documentation;
5. the PR review and any applicable checks pass on the exact final head.

## Mandatory post-merge task

HLS-C016 owns the final frozen-SHA verification/handoff after any accepted C014 merge. It must not reuse pre-merge or predecessor-SHA workflow conclusions. The C014 merge SHA itself must receive all four successful `push/main/exact-SHA` prerequisites before the project can be described as eligible for an authorized trusted-runner formal-release attempt.

## Post-C015 baseline result

Final baseline refresh before the canonical transition:

- `main` compared **identical** to `d37e8cac666c3ebd7f3c8ffa326a15321ef76185`;
- `v7 CI` #527: `push/main/exact-SHA`, **success**;
- `v7 Candidate Package` #175: `push/main/exact-SHA`, **success**; build, package-size audit and artifact upload completed;
- `Maintenance Security` #70: `push/main/exact-SHA`, **success**;
- `Rust Security` #44: `push/main/exact-SHA`, **success**.

Canonical v7.0.2 remained 28/28 verified with zero partial/blocked features, and no new source/security/governance blocker was reproduced.

## Canonical readiness decision

**AUTHORIZED_FOR_CANONICAL_READINESS_TRANSITION, PENDING FINAL PR-HEAD CHECKS/REVIEW.**

The repository-level decision conditions are satisfied. Commit `73cde20466d5218b55f1869ded79a08886bb6bc2` therefore changes only the canonical lifecycle metadata:

- `release_ready: false -> true`
- `audit_state: v7_0_2_iteration_in_progress -> v7_0_2_release_specialties_verified`

No feature entry, product version, summary, generated-from reference, runtime source, workflow, build script, tag, release, signing state or publication state is changed by that commit.

This is not yet task completion or merge authorization. Because `feature-parity.json` is inside both v7 CI and Candidate pull-request path filters, the **final PR #79 exact head** must receive its own applicable checks and exact-head fallback review. Any later head movement invalidates a previous review/check set. After merge, HLS-C016 must ignore all predecessor-SHA workflow successes and validate the actual merge SHA from scratch.

Coordination state includes planned P0 HLS-C016, preserving two unfinished tasks for one active worker. An intermediate registry commit briefly compressed old completed-task evidence while adding C016; the immediately following correction restored the prior machine-readable evidence and kept the C014/C016 additions. That intermediate commit must not be mistaken for intentional historical-state deletion.
