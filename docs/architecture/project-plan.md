# Current execution plan — v7.0.2

This plan is the live coordinator route. Historical implementation detail remains in `docs/v7-refinement-plan.md`, `docs/v7-iteration-log.md`, task worker logs, and the architecture audit documents.

## Current baseline

- Product iteration: v7.0.2.
- Canonical `artifacts/v7-productization/feature-parity.json`: 28/28 verified, 0 partial, 0 blocked. C014 has now committed the reviewed lifecycle metadata transition to `release_ready=true` and `audit_state=v7_0_2_release_specialties_verified`; task completion still depends on final PR-head checks/review.
- HLS-C013 / PR #75 completed the final source/CI audit and found no reproduced deterministic product or release-workflow blocker.
- HLS-C015 / PR #78 is complete. Its first head was rejected for a stale live project plan; the corrected exact head `c547b0ed3ef2a4837b27478aada0c996f42bc518` passed fallback review and merged as `d37e8cac666c3ebd7f3c8ffa326a15321ef76185`.
- HLS-C014 is the only active task. Its post-C015 main baseline remained exactly `d37e8cac666c3ebd7f3c8ffa326a15321ef76185` through the decision checkpoint.
- All four post-C015 baseline prerequisites succeeded on that exact main SHA: v7 CI #527, v7 Candidate Package #175, Maintenance Security #70, Rust Security #44.
- Commit `73cde20466d5218b55f1869ded79a08886bb6bc2` changes only canonical readiness lifecycle metadata: `release_ready false->true` and `audit_state v7_0_2_iteration_in_progress->v7_0_2_release_specialties_verified`.
- HLS-C014 is now waiting for the applicable v7 CI / Candidate pull-request workflows and exact-head fallback audit on PR #79. The transition is not merge-authorized until those final-head checks pass.
- HLS-C016 is planned P0 as the mandatory post-C014 frozen-SHA verification/handoff task. With one active worker, C014 + C016 preserve the minimum two unfinished tasks.

## Route principles

1. Preserve current v7 architecture unless a reproducible defect requires change.
2. Treat the final reviewed `main` SHA as the releasable unit; any later `main` movement requires a fresh four-workflow exact-SHA set.
3. Separate source/CI defects from trusted-runner, signing, browser, environment, approval, and operator prerequisites.
4. Keep candidate and formal-release semantics distinct; green candidate CI does not by itself authorize formal publication.
5. Canonical `release_ready=true` is repository-governance permission to enter formal gates, not evidence that those gates or publication controls already passed.
6. Prefer narrow branches and multiple meaningful commits. A durable audit FAIL must be resolved or explicitly rebutted before merge; any head movement invalidates an older exact-head PASS.
7. Keep at least twice as many unfinished tasks as active workers except at genuine project closeout.
8. After a final release SHA is frozen, do not merge coordination-only evidence back to `main` before the formal-release attempt; that would move main and invalidate the frozen exact-SHA workflow set.

## Completed phases

### Collaboration and release-integrity bootstrap

HLS-C001 established the shared task registry, role/heartbeat/visitor issues, manager log, worker-log convention, handoff and plan. HLS-C002 fixed and audited the formal release prerequisite contract so every `main` commit receives all four prerequisite workflows and the release job accepts only canonical workflow name/path + `push` + `main` + exact SHA matches.

### Formal-package version integrity

HLS-C007 removed stale executable v7.0.1 assumptions from the v7.0.2 MSI lifecycle gate, bound candidate/MSI identity to canonical product metadata, and preserved the pinned public v7.0.0 upgrade baseline and lifecycle protections.

### Release-boundary triage and dependency maintenance

HLS-C003 documented the source-versus-external formal release boundary. HLS-C004 reconciled the actual dependency queue and corrected the stale Dependabot statement that had caused the PR #64 process deviation.

### Cross-cutting hardening

HLS-C005 reproduced three concrete defects and split them into narrow fixes:

- HLS-C009 / PR #68: bind automatic updates to project-owned signer identity/explicit rollover trust;
- HLS-C010 / PR #67: enforce loopback-only optional Core TCP transport, including the actual pre-bound listener;
- HLS-C011 / PR #69: prevent replay-controlled custom credential headers from crossing origins before exact target-origin context restoration.

All are merged. Later changes through the C013 audited source did not overwrite these product-security implementations.

### Documentation and durable-state closeout

HLS-C006/PR #70 refined README accuracy. HLS-C008/PR #72 reconciled current v7.0.2 release/install/branch documentation while preserving v7.0.1 evidence as historical. HLS-C012/PR #73 synchronized durable coordination state.

### Final source-readiness re-audit

HLS-C013/PR #75 audited frozen `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`. That SHA had successful v7 CI #525, v7 Candidate Package #173, Maintenance Security #68 and Rust Security #42 push/main/exact-SHA evidence, the canonical matrix was 28/28 verified, and no C009-C011 hardening was overwritten. C013 passed under the single-participant exact-head fallback, but did not authorize publication.

### Live governance reconciliation

HLS-C015/PR #78 resolved the remaining live instruction contradiction discovered by C014. Root `AGENTS.md` now names v7.0.2 as active and canonical feature-parity metadata as version/readiness truth. It also states that formal packaging requires fully verified parity, an explicitly reviewed `release_ready=true` decision, and the visual/performance/installer/rollback gates; candidate CI is not publication authorization. The first PR head was correctly rejected because this live plan had not been updated, then the corrected head was re-reviewed from scratch and merged.

## Active phase — HLS-C014 canonical readiness decision

Priority: P0.

Repository history establishes the readiness lifecycle precedent: the v7.0.1 ready state used `release_ready=true` with `audit_state=v7_0_1_release_specialties_verified`; the automated start of the v7.0.2 iteration reset those two fields together to `false` and `v7_0_2_iteration_in_progress`.

C014 treats `release_ready=true` as repository-governance authorization to enter the existing formal package/release pipeline, not as evidence that the external trusted Windows runner, browsers, MSI lifecycle/rollback evidence, signing identity, protected environment or publication approval have already passed.

The transition decision conditions were satisfied before commit `73cde204...`:

1. canonical v7.0.2 remained 28/28 verified, 0 partial, 0 blocked;
2. C013's source/security conclusions remained applicable and no new blocker was reproduced;
3. C015's live-governance reconciliation remained current;
4. post-C015 main stayed exactly `d37e8cac...` while all four required main-push workflows completed successfully;
5. the readiness transition changes no executable release gate;
6. HLS-C016 is already defined to verify the eventual C014 merge SHA from scratch before any formal dispatch.

The resulting narrow transition is:

- `release_ready: false -> true`
- `audit_state: v7_0_2_iteration_in_progress -> v7_0_2_release_specialties_verified`

No feature entry, product version, summary, generated-from reference, workflow, runtime, build script, tag, release, signing state or publication state changes.

### Remaining C014 acceptance work

PR #79 must now become stable on its final exact head and satisfy applicable pull-request checks. Both v7 CI and v7 Candidate Package path filters include `artifacts/v7-productization/feature-parity.json`, so absence of checks would itself require investigation rather than a docs-only waiver.

After applicable checks succeed, the auditor fallback must inspect the final exact head, verify the feature-parity patch is only the two intended metadata fields, verify manager history is append-only and task/handoff/plan state is coherent, and confirm `main` has not moved from the PR base. Any head movement invalidates the review and check set.

Only then may PR #79 merge.

## Next phase — HLS-C016 frozen-SHA verification and trusted-release handoff

After an accepted C014 merge, freeze that exact new `main` **merge SHA**. HLS-C016 must reject predecessor-SHA and PR-head successes and require all four of these `push/main/exact-SHA` workflows to complete successfully on the C014 merge SHA:

- v7 CI;
- v7 Candidate Package;
- Maintenance Security;
- Rust Security.

C016 must then reconfirm `main` is still identical and canonical v7.0.2 remains 28/28 verified with `release_ready=true`. A failure creates a new source-fix task rather than a bypass.

C016 itself does not tag, sign, create a release, dispatch the formal workflow or publish. It hands the frozen source to the external trusted-release boundary: self-hosted Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, candidate-bound visual/performance/MSI/rollback evidence, signing tool and trusted project certificate/private key, timestamp/network access, protected `v7-release` environment/approval, draft asset digest verification, and explicit operator publish choice.

Because any later `main` commit creates a new prospective release SHA, C016 evidence should remain in Issues and, if a repository worker log is useful, on an **unmerged** evidence branch while the frozen SHA is intended for formal release.

## Dynamic replanning

- Any new reproducible source/security/governance contradiction becomes a separate P0/P1 task ahead of merge/release.
- A failed required workflow is investigated rather than waived.
- A cancelled run from an obsolete SHA after a PR head or `main` moves is not a product regression, but it cannot authorize merge/release.
- External runner/signing/browser/operator absence remains an external blocker; repository code must not simulate or weaken those trust boundaries.
- A returning worker must redeclare and heartbeat before work is assigned. If additional workers become active, expand the unfinished backlog back to at least 2× active worker count unless the project has genuinely entered closeout.

Every route change is mirrored in Issue #39, `docs/coordination/tasks.json`, `handoff.md`, `docs/manger.log`, and this plan when it changes the project route.
