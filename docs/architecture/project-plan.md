# Current execution plan — v7.0.2

This plan is the live coordinator route. Historical implementation detail remains in `docs/v7-refinement-plan.md`, `docs/v7-iteration-log.md`, task worker logs, and the architecture audit documents.

## Current baseline

- Product iteration: v7.0.2.
- Canonical `artifacts/v7-productization/feature-parity.json`: 28/28 verified, 0 partial, 0 blocked, `release_ready=false`.
- HLS-C001 through HLS-C012 are complete. C012/PR #73 merged as `1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`.
- HLS-C013 audits that frozen post-closeout SHA. Its four required main-push workflows are all successful: v7 CI #525, v7 Candidate Package #173, Maintenance Security #68, Rust Security #42.
- The formal release workflow and package verifier still fail closed on exact-SHA provenance, current-main drift, trusted runner/browser/signing prerequisites, canonical completeness, release evidence, clean worktree, and `release_ready=true`.
- No current task authorizes a tag, GitHub Release, signing bypass, formal publish, or automatic `release_ready` change.

## Route principles

1. Preserve current v7 architecture unless a reproducible defect requires change.
2. Treat the final reviewed `main` SHA as the releasable unit; any later `main` movement requires a fresh four-workflow exact-SHA set.
3. Separate source/CI defects from trusted-runner, signing, browser, environment, approval, and operator prerequisites.
4. Keep candidate and formal-release semantics distinct; green candidate CI does not by itself authorize formal publication.
5. Change canonical `release_ready` only through an explicit separately reviewed readiness decision.
6. Prefer narrow branches and multiple meaningful commits. A durable audit FAIL must be resolved or explicitly rebutted before merge; any head movement invalidates an older exact-head PASS.
7. Keep at least twice as many unfinished tasks as active workers except at genuine project closeout. With one active worker, C013 + C014 maintain the current minimum of two.

## Completed phases

### Collaboration and release-integrity bootstrap

HLS-C001 established the shared task registry, role/heartbeat/visitor issues, manager log, worker-log convention, handoff and plan. HLS-C002 then fixed and audited the formal release prerequisite contract so every `main` commit receives all four prerequisite workflows and the release job accepts only canonical workflow name/path + `push` + `main` + exact SHA matches.

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

HLS-C006/PR #70 refined README accuracy. HLS-C008/PR #72 reconciled current v7.0.2 release/install/branch documentation while preserving v7.0.1 evidence as historical. HLS-C012/PR #73 synchronized the machine-readable registry, handoff, manager log and worker state after those merges.

## Active phase — HLS-C013 final source-readiness re-audit

Priority: P2 because implementation/security blockers are already closed; it is nevertheless the only active task and blocks the next governance decision.

Acceptance requires:

1. audit the C012 merge SHA against the executable formal-release contract;
2. verify all four required `push/main/exact-SHA` workflows before classification;
3. independently confirm C009-C011 hardening was not overwritten by later source changes;
4. separate reproducible source blockers from external trusted-release prerequisites;
5. keep `release_ready=false`, create no tag/release/publish action, and weaken no gate;
6. produce either an actionable source-fix route or a governance/external-gate handoff.

Current implementation-side evidence yields **source/CI contract clean at `1c93cffe...`, formal publication still blocked by design**. C013 remains unfinished until its branch receives an exact-head fallback audit and merge decision.

## Next phase — HLS-C014 explicit readiness decision

Priority: P0 after C013 because it controls whether the formal package can ever pass its deliberate policy gate.

C014 must independently revalidate C013 and then make one explicit decision:

- **not authorized**: keep `release_ready=false` and record the concrete missing governance/evidence prerequisite; or
- **authorized**: make the narrow reviewed canonical readiness transition without weakening any executable gate.

If an authorized C014 change merges, freeze that resulting `main` SHA and wait for new successful v7 CI, v7 Candidate Package, Maintenance Security and Rust Security `push/main/exact-SHA` results. Only then may an authorized operator attempt the formal workflow on the dedicated trusted release environment.

The formal workflow must still supply the self-hosted Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, signing tool and trusted project certificate/private key, timestamp/network access, protected `v7-release` environment/approval, evidence/digest checks, and explicit publish choice.

## Dynamic replanning

- Any new reproducible source/security defect becomes a separate P0/P1 task ahead of C014.
- A failed required workflow on the frozen final SHA is investigated before formal release; a cancelled run from an obsolete SHA after `main` moves is not a product regression.
- External runner/signing/browser/operator absence remains an external blocker; repository code must not simulate or weaken those trust boundaries.
- A returning worker must redeclare and heartbeat before work is assigned. If additional workers become active, expand the unfinished backlog back to at least 2× active worker count unless the project has genuinely entered closeout.

Every route change is mirrored in Issue #39, `docs/coordination/tasks.json`, `handoff.md`, and `docs/manger.log`.
