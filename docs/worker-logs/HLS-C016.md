# HLS-C016 — Freeze the accepted readiness SHA and verify trusted-release prerequisites

## Acceptance criteria

1. Start only after HLS-C014 is accepted and merged. Freeze that **exact** C014 merge SHA as the prospective release source; predecessor-SHA workflow results are not release evidence.
2. Require completed/successful `push/main/exact-SHA` results for all four canonical prerequisite workflows on the frozen SHA: `v7 CI`, `v7 Candidate Package`, `Maintenance Security`, and `Rust Security`.
3. Reconfirm remote `main` is still identical to the frozen SHA before classifying it as eligible for an authorized trusted-runner formal-release attempt.
4. Confirm canonical metadata on the frozen SHA is v7.0.2, 28/28 verified with 0 partial/blocked entries, and `release_ready=true` if HLS-C014 authorized the transition.
5. Do not tag, sign, create/publish a release, or dispatch the formal workflow. Handoff only to the dedicated Windows `hls-release` runner/signing/browser/environment/operator boundary; if any reproducible source or prerequisite-CI failure appears, create a source-fix task instead of weakening a gate.
6. While the frozen SHA is intended for formal release, do **not** merge C016 evidence/coordination commits into `main`. Any such merge would create a new prospective release SHA and invalidate the existing exact-SHA workflow set.

## Pre-initialized assignment contract

- Priority: P0
- Planned owner: unassigned until HLS-C014 completes; an idle eligible worker may self-claim immediately afterward.
- Role: coordinator + auditor (worker fallback allowed under the repository protocol if no independent participant is available).
- Dependency: HLS-C014.
- Canonical task registry: Issue #39 and `docs/coordination/tasks.json`.
- Heartbeat: Issue #41 while the task is active.

## Freeze-safe evidence procedure

1. Claim HLS-C016 in Issue #39 and heartbeat in Issue #41.
2. Read the accepted HLS-C014 PR and obtain its **merge commit**, not merely the reviewed PR head.
3. Confirm `main == <C014 merge SHA>` before collecting workflow evidence.
4. Query each required workflow by exact head SHA and verify canonical workflow path, `event=push`, `head_branch=main`, `status=completed`, `conclusion=success`.
5. Re-read canonical feature parity from the frozen main SHA and confirm the readiness state authorized by C014.
6. Perform a final `frozen_sha...main` compare. If main moved, stop and restart the freeze on the new reviewed main state; do not reuse old workflow conclusions.
7. Record evidence in Issue #39/Issue #41. If a repository log commit is useful, create a dedicated evidence branch from the frozen SHA and keep it **unmerged** until the formal-release attempt is completed or abandoned.
8. Report one of two outcomes:
   - `ELIGIBLE_FOR_AUTHORIZED_TRUSTED_FORMAL_ATTEMPT`: source/readiness and four exact-SHA prerequisite workflows pass; external trust/operator controls are still required.
   - `BLOCKED`: name the exact failed/missing source or prerequisite-CI condition and create/reprioritize a fix task when it is repository-owned.

## External trust boundary that C016 must not fake

- dedicated self-hosted Windows x64 runner labeled `hls-release`;
- fixed `E:\h` lifecycle environment;
- real Edge and Firefox installations;
- signing tool and trusted project code-signing certificate/private key;
- timestamp/network access;
- protected `v7-release` environment/approval controls;
- candidate-bound visual/performance/MSI-upgrade/forced-rollback evidence;
- draft asset byte-size/SHA-256 verification;
- explicit operator choice to publish.

C016 verifies eligibility to enter that boundary. It is not itself a release/publication authorization.
