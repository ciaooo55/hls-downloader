# Current execution plan — v7.0.2

This plan is the live coordinator route. Historical implementation detail remains in `docs/v7-refinement-plan.md`, `docs/v7-iteration-log.md`, task worker logs, and the architecture audit documents.

## Current baseline

- Product iteration: canonical v7.0.2.
- Canonical `artifacts/v7-productization/feature-parity.json`: 28/28 verified, 0 partial, 0 blocked, `release_ready=false`.
- HLS-C001 through HLS-C012 are complete. C012/PR #73 merged as `1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`.
- HLS-C013 audited that frozen post-closeout SHA. Its four required main-push workflows all succeeded: v7 CI #525, v7 Candidate Package #173, Maintenance Security #68, Rust Security #42. `worker-1` independently revalidated that source-level conclusion.
- PR #75 merged the C013 evidence branch to `main` as `718ee541ce584a4ac229b120a9a478583fc07c0a` before the active independent delivery review was complete. This is an explicit coordination/process deviation under HLS-C015; it does not invalidate the reproduced source evidence, but it blocks later readiness governance until the audit chain is reconciled.
- Root `AGENTS.md` still says the only active version is `7.0.1`, contradicting canonical v7.0.2 metadata. This is queued separately as HLS-C016 because repository instructions are an operational contract for future agents/maintainers; historical v7.0.1 release/baseline evidence must remain historical rather than being globally rewritten.
- The formal release workflow and package verifier still fail closed on exact-SHA provenance, current-main drift, trusted runner/browser/signing prerequisites, canonical completeness, release evidence, clean worktree, and `release_ready=true`.
- No current task authorizes a tag, GitHub Release, signing bypass, formal publish, or automatic `release_ready` change.

## Route principles

1. Preserve current v7 architecture unless a reproducible defect requires change.
2. Treat the final reviewed `main` SHA as the releasable unit; any later `main` movement requires a fresh four-workflow exact-SHA set.
3. Separate source/CI defects from trusted-runner, signing, browser, environment, approval, operator, documentation, and collaboration-state defects so fixes remain narrow.
4. Keep candidate and formal-release semantics distinct; green candidate CI does not by itself authorize formal publication.
5. Change canonical `release_ready` only through an explicit separately reviewed readiness decision.
6. Prefer narrow branches and multiple meaningful commits. A durable audit FAIL must be resolved or explicitly rebutted before merge; any head movement invalidates an older exact-head PASS.
7. When a merge violates rule 6, stop later dependent work, create a narrow process-reconciliation task, preserve the factual merge history, and restore machine/log/handoff consistency before resuming the route.
8. When a new verified defect changes the route, create a new task ID instead of silently expanding a task already under review.
9. Keep at least twice as many unfinished tasks as active workers except at genuine project closeout. C015/C016/C014 are closeout work; do not manufacture filler tasks solely for a numeric target.

## Completed technical phases

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

### HLS-C013 source-readiness evidence

The audited source was frozen at `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`.

Source acceptance evidence established:

1. all four required `push/main/exact-SHA` prerequisite workflows succeeded;
2. C009-C011 hardening was not overwritten by later source changes;
3. canonical feature parity remained 28/28 verified with `release_ready=false`;
4. formal packaging still independently requires canonical completeness, `release_ready=true`, clean worktree and release evidence;
5. external trusted-runner/browser/signing/environment/operator requirements remain separate and mandatory.

`worker-1` independently issued a source-level PASS. PR #75 then merged the C013 branch before `worker-1` issued the required exact-head **delivery** PASS and while a durable changes-required finding about coordination state was still active. Therefore C013's technical conclusion remains usable, but its collaboration/acceptance closeout is intentionally held open through HLS-C015.

## Active phase — HLS-C015 process reconciliation

Priority: P0. This interrupts later work because the repository protocol explicitly forbids merging through an unresolved durable audit FAIL.

HLS-C015 must:

1. record PR #75 merge `718ee541ce584a4ac229b120a9a478583fc07c0a` and the unresolved independent-review state factually;
2. restore machine task state to protocol-valid values and distinguish `worker-0` task/evidence ownership from `worker-1` independent audit responsibility;
3. remove stale handoff claims that no independent auditor is active;
4. append, not rewrite, coordinator-log entries covering `worker-1` re-entry, independent source PASS, helper PR #74/#76, delivery rejection, PR #75 premature merge, and this corrective route;
5. keep the correction documentation/coordination-only and preserve `release_ready=false` plus all executable release gates;
6. receive a fresh `worker-1` exact-head review before its main PR may merge.

PR #77 is the dedicated main-based correction PR. HLS-C016 and HLS-C014 must not start implementation on stale bases before C015 is accepted and merged.

## Next phase — HLS-C016 root instruction-version reconciliation

Priority: P0 after C015 and before release-readiness governance.

The current root `AGENTS.md` says `The only active product version is 7.0.1`, while canonical feature-parity metadata says `product_version=7.0.2`. This is a repository-instruction/current-state contradiction that can misroute future agents even though it does not alter executable release behavior.

HLS-C016 must:

1. start from the reviewed post-C015 `main`;
2. update the active-version instruction to canonical v7.0.2;
3. search instruction/current-state surfaces for the same active-version contradiction while preserving legitimate historical v7.0.1 candidate/baseline references;
4. maintain a dedicated `docs/worker-logs/HLS-C016.md` with source-of-truth and validation evidence;
5. remain docs/instruction-only and leave canonical `release_ready=false` plus executable gates untouched;
6. receive `worker-1` exact-head independent review before merge.

HLS-C014 remains paused behind both HLS-C015 and HLS-C016.

## Final governance phase — HLS-C014 explicit readiness decision

Priority: P0 only **after HLS-C015 and HLS-C016 are accepted and merged**.

C014 must independently revalidate the accepted C013/C015/C016 state and then make one explicit decision:

- **not authorized**: keep `release_ready=false` and record the concrete missing governance/evidence prerequisite; or
- **authorized**: make the narrow reviewed canonical readiness transition without weakening any executable gate.

If an authorized C014 change merges, freeze that resulting `main` SHA and wait for new successful v7 CI, v7 Candidate Package, Maintenance Security and Rust Security `push/main/exact-SHA` results. Only then may an authorized operator attempt the formal workflow on the dedicated trusted release environment.

The old successful workflow set for `1c93cffe...` remains valid historical C013 source-audit evidence, but because `main` has moved it can never authorize a formal release of `718ee541...` or any later commit.

The formal workflow must still supply the self-hosted Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, signing tool and trusted project certificate/private key, timestamp/network access, protected `v7-release` environment/approval, evidence/digest checks, and explicit publish choice.

## Dynamic replanning

- Any new reproducible source/security defect becomes a separate P0/P1 task ahead of C014.
- A failed required workflow on the frozen final SHA is investigated before formal release; a cancelled run from an obsolete SHA after `main` moves is not a product regression.
- External runner/signing/browser/operator absence remains an external blocker; repository code must not simulate or weaken those trust boundaries.
- A returning worker must redeclare and heartbeat before work is assigned. If additional ordinary workers become active, expand the unfinished backlog back to at least 2× active worker count unless the project has genuinely entered closeout.
- A dependent task is interrupted immediately when a higher-priority audit, instruction-contract fix, or process correction becomes active; resume it only after the blocking task is durably accepted.

Every route change is mirrored in Issue #39, `docs/coordination/tasks.json`, `handoff.md`, and `docs/manger.log`.
