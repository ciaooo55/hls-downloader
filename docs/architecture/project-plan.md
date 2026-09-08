# Current execution plan — v7.0.2

This plan is the live coordinator route. Historical implementation detail remains in `docs/v7-refinement-plan.md`, `docs/v7-iteration-log.md`, task worker logs, and the architecture audit documents.

## Current baseline

- Product iteration: v7.0.2.
- Canonical `artifacts/v7-productization/feature-parity.json`: 28/28 verified, 0 partial, 0 blocked, `release_ready=false`.
- HLS-C013 is complete. PR #75 accepted exact head `d6221b55d28e1d9c596840637db9e1e4ff0287f9` and merged as `718ee541ce584a4ac229b120a9a478583fc07c0a`.
- C013 classified its frozen source/CI contract as clean while explicitly leaving formal publication blocked by the canonical readiness gate and external trusted-release prerequisites.
- HLS-C014 is the explicit readiness-decision task, but it is temporarily waiting on HLS-C015 because its pre-audit reproduced a live governance contradiction: root `AGENTS.md` still declared v7.0.1 while canonical/current state is v7.0.2.
- HLS-C015 is the only active task. It reconciles that live instruction and the corresponding stale documentation observation without touching `release_ready`, product code, workflows, build scripts, tags, releases or publication state.

## Route principles

1. Preserve current v7 architecture unless a reproducible defect requires change.
2. Treat the final reviewed `main` SHA as the releasable unit; any later `main` movement requires a fresh four-workflow exact-SHA set.
3. Separate source/CI defects from trusted-runner, signing, browser, environment, approval, and operator prerequisites.
4. Keep candidate and formal-release semantics distinct; green candidate CI does not by itself authorize formal publication.
5. Change canonical `release_ready` only through an explicit separately reviewed readiness decision after live governance instructions are internally consistent.
6. Prefer narrow branches and multiple meaningful commits. A durable audit FAIL must be resolved or explicitly rebutted before merge; any head movement invalidates an older exact-head PASS.
7. Keep at least twice as many unfinished tasks as active workers except at genuine project closeout. With one active worker, HLS-C015 plus waiting HLS-C014 preserve the current minimum of two.

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

All are merged. Later changes before the C013 frozen SHA touched documentation/coordination only, not product/runtime/workflow/script/feature-parity source.

### Documentation and durable-state closeout

HLS-C006/PR #70 refined README accuracy. HLS-C008/PR #72 reconciled current v7.0.2 release/install/branch documentation while preserving v7.0.1 evidence as historical. HLS-C012/PR #73 synchronized durable coordination state.

### Final source-readiness re-audit

HLS-C013/PR #75 independently re-read the executable release/package contract and audited frozen `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`. That SHA had successful v7 CI #525, v7 Candidate Package #173, Maintenance Security #68 and Rust Security #42 push/main/exact-SHA evidence, the canonical matrix was 28/28 verified, and no C009-C011 hardening was overwritten. C013 passed under the single-participant exact-head fallback, but did not authorize publication or change `release_ready=false`.

## Active phase — HLS-C015 live governance reconciliation

Priority: P0 because a readiness decision must not proceed while live repository instructions contradict canonical product metadata.

Acceptance requires:

1. root `AGENTS.md` identifies v7.0.2 as the active product version and canonical feature-parity metadata as the version/readiness source of truth;
2. formal-package instructions require fully verified parity, an explicit reviewed `release_ready=true` decision, and visual/performance/installer/rollback gates, while making clear candidate CI is not publication authorization;
3. `docs/architecture/formal-release-readiness.md` no longer carries known current-facing v7.0.1 wording as an open issue after HLS-C008/C015, while preserving historical evidence;
4. no product/runtime/workflow/build-script/feature-parity/tag/release/publish change;
5. exact-head fallback audit of the dedicated docs/governance PR, with no invented CI success if path filtering produces no PR checks.

The first PR review of HLS-C015 found this project plan itself stale because it still called C013 active and C014 next. That head is not accepted. The plan is now being corrected on the same branch and the PR must be re-audited from its new exact head.

## Next phase — HLS-C014 explicit readiness decision

After HLS-C015 is reviewed and merged, HLS-C014 resumes from the resulting new `main` baseline. Its pre-existing branch contains no readiness-state commit and must be fast-forwarded or recreated from that baseline before implementation.

C014 must independently revalidate source/governance evidence and then make one explicit decision:

- **not authorized**: keep `release_ready=false` and record the concrete missing governance/evidence prerequisite; or
- **authorized**: make a narrow reviewed canonical readiness transition without weakening any executable gate.

If an authorized C014 change merges, freeze that resulting `main` SHA and require new successful v7 CI, v7 Candidate Package, Maintenance Security and Rust Security `push/main/exact-SHA` results. Only after that exact-SHA set succeeds may an authorized operator attempt the formal workflow on the dedicated trusted release environment.

The formal workflow must still supply the self-hosted Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, signing tool and trusted project certificate/private key, timestamp/network access, protected `v7-release` environment/approval, evidence/digest checks, and explicit publish choice.

## Dynamic replanning

- Any new reproducible source/security/governance contradiction becomes a separate P0/P1 task ahead of the readiness transition.
- A failed required workflow on the frozen final SHA is investigated before formal release; a cancelled run from an obsolete SHA after `main` moves is not a product regression.
- External runner/signing/browser/operator absence remains an external blocker; repository code must not simulate or weaken those trust boundaries.
- A returning worker must redeclare and heartbeat before work is assigned. If additional workers become active, expand the unfinished backlog back to at least 2× active worker count unless the project has genuinely entered closeout.

Every route change is mirrored in Issue #39, `docs/coordination/tasks.json`, `handoff.md`, `docs/manger.log`, and this plan when it changes the project route.
