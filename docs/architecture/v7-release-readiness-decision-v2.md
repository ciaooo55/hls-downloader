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

The exact post-C017 main SHA `426fc8eed97339885be16bf0ef5fff386a38bd31` received all four required `push/main/exact-SHA` successes before any readiness transition was committed:

- v7 CI #534 — SUCCESS
- v7 Candidate Package #182 — SUCCESS
- Maintenance Security #71 — SUCCESS
- Rust Security #45 — SUCCESS

Remote `main` remained exactly `426fc8eed97339885be16bf0ef5fff386a38bd31` through the decision checkpoint. Predecessor-SHA successes, including the pre-C017 `d37e8cac...` set, are supporting history only and do not authorize this transition.

## Decision basis retained from C013/C015/C017

1. C013 independently established a clean source/CI contract with 28/28 verified and no reproduced deterministic product/release-workflow blocker.
2. C015 reconciled the live v7.0.2 governance instructions without changing product/runtime/workflow/build-script/readiness state.
3. C017 reconciled delivery-process races, task-ID collisions and heartbeat-state errors before this governance decision resumed.
4. A compare from the C013 frozen source to post-C017 main contains governance/documentation changes only; no runtime, workflow, build script or canonical feature-parity change occurred in that interval.
5. Executable formal gates remain fail-closed: canonical completeness, `release_ready=true`, clean worktree, candidate-bound release evidence, exact-SHA prerequisites, current-main drift protection, trusted Windows runner, real browsers, MSI/rollback/performance evidence, project signer identity, draft asset digest verification and explicit publication choice.

## Audit-state semantic correction

The superseded PR #79 proposed `audit_state=v7_0_2_release_specialties_verified` by copying the prior v7.0.1 ready label. A history audit showed that label first entered v7.0.1 in commit `d7ebd90b8dcaa8b9426690415081caaf49a49a63` together with actual focused/browser release-specialty evidence and the final 28/28 transition. Reusing that label for v7.0.2 before the trusted-runner browser/performance/MSI/rollback gates run would overstate current evidence.

The repository's current lifecycle helper `scripts/bump-v7-version.ps1` defines the canonical ready-state label generically as `v<version>_release_ready`; for v7.0.2 that is `v7_0_2_release_ready`. Executable package/release gates consume the boolean readiness/completeness/evidence contract rather than requiring the historical `release_specialties_verified` string.

Therefore C014 uses the narrower truthful lifecycle label and leaves release-specialty verification to the trusted formal-release path.

## Authorized transition

The post-C017 baseline refresh is fully successful and the source/governance conditions remain satisfied. C014 therefore authorizes only:

- `release_ready: false -> true`
- `audit_state: v7_0_2_iteration_in_progress -> v7_0_2_release_ready`

No feature entry, product version, verification statement, runtime/workflow/build script, tag, release or publication state may change in that transition.

`release_ready=true` means the source may enter the existing formal validation pipeline. It does **not** mean v7.0.2 browser/performance/MSI/rollback/signing/publication evidence has already passed.

## Review and post-merge rule

The final C014 PR head must receive applicable exact-head checks/review. Any head movement invalidates prior authorization.

If C014 is accepted and merged, HLS-C016 owns the resulting exact **merge SHA**. C016 must require a new four-workflow `push/main/exact-SHA` SUCCESS set for that merge SHA and reconfirm remote main is unchanged before classifying it eligible for an authorized trusted-runner formal-release attempt.

C016 evidence must not itself be merged into main while that SHA is intended for release, because such a merge would change the prospective release SHA.

## External boundary

Even with canonical readiness set true, formal release remains blocked until the trusted external boundary is actually present: dedicated Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, signing tool and project certificate/private key, timestamp/network trust, protected `v7-release` environment/approval, candidate-bound visual/performance/MSI/rollback evidence, draft asset digest verification and explicit operator publication choice.
