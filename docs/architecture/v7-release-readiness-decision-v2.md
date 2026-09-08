# v7.0.2 canonical readiness decision — post-C017 baseline

## Purpose

HLS-C014 decides whether canonical v7.0.2 may move from iteration state into the existing formal validation/release pipeline. This is a repository governance/source-state decision only. It does not tag, sign, dispatch, create a GitHub Release or publish.

## Clean baseline

C017/PR #80 reconciled the concurrent audit/task-state races and merged as `main@426fc8eed97339885be16bf0ef5fff386a38bd31`. The earlier C014 draft PR #79 is closed unmerged and is historical evidence only; its stale coordination snapshots and predecessor-SHA workflow evidence cannot authorize this decision.

Canonical main at the start of this v2 decision remains:

- product version: `7.0.2`
- feature matrix: 28/28 verified, zero partial/blocked
- `release_ready=false`
- `audit_state=v7_0_2_iteration_in_progress`

## Required baseline refresh

The exact post-C017 main SHA `426fc8eed97339885be16bf0ef5fff386a38bd31` must receive all four prerequisite `push/main/exact-SHA` successes before the readiness transition may be committed:

- v7 CI #534 — pending at task restart
- v7 Candidate Package #182 — pending at task restart
- Maintenance Security — pending at task restart
- Rust Security — pending at task restart

Predecessor-SHA successes, including the pre-C017 `d37e8cac...` set, are supporting history only and cannot replace this refresh.

## Decision basis retained from C013/C015/C017

1. C013 independently established a clean source/CI contract with 28/28 verified and no reproduced deterministic product/release-workflow blocker.
2. C015 reconciled the live v7.0.2 governance instructions without changing product/runtime/workflow/build-script/readiness state.
3. C017 reconciled delivery-process races, task-ID collisions and heartbeat-state errors before this governance decision resumed.
4. Executable formal gates remain fail-closed: canonical completeness, `release_ready=true`, clean worktree, candidate-bound release evidence, exact-SHA prerequisites, current-main drift protection, trusted Windows runner, real browsers, MSI/rollback/performance evidence, project signer identity, draft asset digest verification and explicit publication choice.

## Proposed transition

No transition is authorized until the post-C017 baseline refresh above is fully successful. If it succeeds and the final exact PR diff remains narrow, C014 may propose only:

- `release_ready: false -> true`
- `audit_state: v7_0_2_iteration_in_progress -> v7_0_2_release_specialties_verified`

No feature entry, product version, verification statement, runtime/workflow/build script, tag, release or publication state may change in that transition.

## Review and post-merge rule

The final C014 PR head must receive applicable exact-head checks/review. Any head movement invalidates prior authorization.

If C014 is accepted and merged, HLS-C016 owns the resulting exact **merge SHA**. C016 must require a new four-workflow `push/main/exact-SHA` SUCCESS set for that merge SHA and reconfirm remote main is unchanged before classifying it eligible for an authorized trusted-runner formal-release attempt.

C016 evidence must not itself be merged into main while that SHA is intended for release, because such a merge would change the prospective release SHA.

## External boundary

Even with canonical readiness set true, formal release remains blocked until the trusted external boundary is actually present: dedicated Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, signing tool and project certificate/private key, timestamp/network trust, protected `v7-release` environment/approval, candidate-bound visual/performance/MSI/rollback evidence, draft asset digest verification and explicit operator publication choice.
