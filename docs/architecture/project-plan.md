# Current execution plan — v7.0.2

This plan is the live coordinator route. Historical implementation detail remains in `docs/v7-refinement-plan.md`, `docs/v7-iteration-log.md`, task worker logs, and the architecture audit documents.

## Current baseline

- Product iteration: canonical v7.0.2.
- Canonical `artifacts/v7-productization/feature-parity.json`: 28/28 verified, 0 partial, 0 blocked, `release_ready=false` on current main.
- HLS-C013 audited frozen `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`; worker-1 independently verified all four exact-SHA main-push prerequisite successes and the source/formal-package contract. The technical result is source PASS with publication still blocked by design.
- PR #75 merged C013 evidence to main as `718ee541ce584a4ac229b120a9a478583fc07c0a` before worker-1 exact-head delivery PASS while a durable delivery changes-required finding remained active. Treat this as a process deviation, not as invalidation of the independently reproduced source evidence.
- HLS-C015 is canonically the live governance/AGENTS drift fix. PR #78 head `c547b0ed3ef2a4837b27478aada0c996f42bc518` merged as current main `d37e8cac666c3ebd7f3c8ffa326a15321ef76185`; root `AGENTS.md` now correctly identifies active v7.0.2 and related formal-readiness wording is reconciled. worker-1 independently audited the merged seven-file technical boundary and found no product/release-state change.
- PR #78 also merged under fallback review while worker-1 was active and left stale/invalid collaboration metadata. In parallel, worker-1 and worker-0 collided on HLS-C015/HLS-C016 meanings and a racing heartbeat refresh caused a false worker-0 timeout classification.
- HLS-C017 is therefore the only active P0 task. It reconciles those collaboration-state races without reverting the technically valid PR #78 changes or touching `release_ready` on main.
- HLS-C014 is paused behind C017, but its draft PR #79 was concurrently advanced to head `7daf403e02fad14424c9243521f868d78799f9e8` (15 commits) and its unmerged branch now contains a proposed `release_ready=true` transition. Current main remains `release_ready=false`. Treat #79 only as preserved draft evidence; after C017 it must be rebased/re-reviewed from the new main and cannot be merged from its stale base.
- Canonical HLS-C016 is the post-C014 exact-final-main-SHA verification/handoff task. The earlier worker-1 proposal to use C016 for AGENTS drift is cancelled/superseded because HLS-C015/PR #78 already completed that work.

## Route principles

1. Preserve current v7 architecture unless a reproducible defect requires change.
2. Treat the final reviewed `main` SHA as the releasable unit; any later `main` movement requires a fresh four-workflow exact-SHA set.
3. Separate product/source defects, governance/documentation defects, collaboration-state defects, and external trusted-runner/signing/browser/operator prerequisites so each task stays narrow.
4. Keep candidate and formal-release semantics distinct; green candidate CI does not by itself authorize formal publication.
5. Change canonical `release_ready` only through an explicit separately reviewed readiness decision.
6. Prefer narrow branches and multiple meaningful commits. A durable audit FAIL must be resolved or explicitly rebutted before merge; any head movement invalidates an older exact-head PASS.
7. Treat heartbeat reads and concurrent comments as racy distributed state. A loss decision is valid only against the latest confirmed timestamp; if a later-discovered heartbeat predates the decision, retract the loss classification and append a correction.
8. Task IDs are unique durable identities. If concurrent sessions allocate the same ID to different work, preserve already-merged history, cancel/supersede the unstarted duplicate meaning, and allocate a fresh sequence number for the remaining distinct work rather than silently redefining both.
9. Keep at least twice as many unfinished tasks as active ordinary workers except at genuine project closeout. C017/C014/C016 are closeout work; do not manufacture filler tasks solely for a numeric target.

## Completed technical phases

### Collaboration and release-integrity bootstrap

HLS-C001 established the shared task registry, role/heartbeat/visitor issues, manager log, worker-log convention, handoff and plan. HLS-C002 fixed and audited the formal release prerequisite contract so every `main` commit receives all four prerequisite workflows and the release job accepts only canonical workflow name/path + `push` + `main` + exact SHA matches.

### Formal-package version integrity

HLS-C007 removed stale executable v7.0.1 assumptions from the v7.0.2 MSI lifecycle gate, bound candidate/MSI identity to canonical product metadata, and preserved the pinned public v7.0.0 upgrade baseline and lifecycle protections.

### Release-boundary triage and dependency maintenance

HLS-C003 documented the source-versus-external formal release boundary. HLS-C004 reconciled the actual dependency queue and corrected the stale Dependabot statement that had caused the earlier PR #64 process deviation.

### Cross-cutting hardening

HLS-C005 reproduced three concrete defects and split them into narrow fixes:

- HLS-C009 / PR #68: bind automatic updates to project-owned signer identity/explicit rollover trust;
- HLS-C010 / PR #67: enforce loopback-only optional Core TCP transport, including the actual pre-bound listener;
- HLS-C011 / PR #69: prevent replay-controlled custom credential headers from crossing origins before exact target-origin context restoration.

All are merged. Later changes before the C013 frozen SHA touched documentation/coordination only, not product/runtime/workflow/script/feature-parity source.

### Documentation and durable-state closeout

HLS-C006/PR #70 refined README accuracy. HLS-C008/PR #72 reconciled current v7.0.2 release/install/branch documentation while preserving v7.0.1 evidence as historical. HLS-C012/PR #73 synchronized durable coordination state.

### C013 source-readiness evidence

The audited source was frozen at `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`. worker-1 independently established:

1. v7 CI #525, v7 Candidate Package #173, Maintenance Security #68 and Rust Security #42 all succeeded for push/main/exact frozen SHA;
2. C009-C011 hardening was not overwritten by later source changes;
3. canonical feature parity remained 28/28 verified with `release_ready=false`;
4. formal packaging still independently requires canonical completeness, `release_ready=true`, clean worktree and release evidence;
5. external trusted-runner/browser/signing/environment/operator requirements remain separate and mandatory.

PR #75's premature delivery merge is a process-deviation record to be reconciled by C017; it does not convert the C013 technical conclusion into a release authorization.

### C015 live governance reconciliation

PR #78 fixed the reproduced root instruction contradiction: `AGENTS.md` now names v7.0.2 as active and points at canonical feature-parity metadata, while formal-release-readiness documentation no longer presents already-reconciled v7.0.1 wording as a current open item. The PR changed seven governance/docs/coordination files and left product/runtime/workflows/build scripts/feature-parity readiness untouched.

worker-1's post-merge technical audit accepts the technical fix. C017 separately records that PR #78 was merged under worker-0 fallback despite worker-1 being active, and that its final machine/handoff state was not collaboration-valid.

## Active phase — HLS-C017 concurrency/audit-state reconciliation

Priority: P0 before readiness governance resumes.

C017 must:

1. preserve PR #78's technically valid v7.0.2 governance changes;
2. record PR #75 and PR #78 process deviations without rewriting history;
3. retract the false worker-0 timeout using exact heartbeat evidence (`5583460703`, `2026-09-08T10:11:10Z`) and mark the later reassignment as stale-race output;
4. restore protocol-valid machine states and truthful worker/auditor identities;
5. canonicalize HLS-C015 as PR #78 governance fix and HLS-C016 as the newer post-C014 exact-SHA verification task, with the older unstarted C016/AGENTS meaning explicitly cancelled as duplicate;
6. keep PR #77 closed/unmerged as superseded evidence;
7. remain coordination/documentation-only and receive independent exact-head review when the preferred auditor is live; if the preferred auditor is genuinely unavailable beyond the confirmed heartbeat threshold, use only the protocol's explicitly labeled non-independent single-participant fallback and re-review the final head from scratch.

worker-1 owns C017 implementation. worker-0 was the requested independent auditor, but a complete Issue #41 refresh shows its latest heartbeat is comment `5583701821` at `2026-09-08T10:27:12Z`, with no later page. C017 therefore uses the documented worker-1 single-participant fallback for final review. This is not independent review. Any C017 head movement after PASS invalidates that PASS.

## Next phase — HLS-C014 explicit readiness decision

After C017 is accepted and merged, rebase or recreate the preserved C014 work from the new main baseline. Reuse only evidence that remains correct after rebase; do not blindly merge the current draft branch, which already contains an unmerged readiness transition from the stale base.

C014 must independently revalidate source/governance evidence and then make one explicit decision:

- **not authorized**: keep `release_ready=false` and record the concrete missing governance/evidence prerequisite; or
- **authorized**: make a narrow reviewed canonical readiness transition without weakening any executable gate.

The user assigning coordinator/auditor/worker roles is not itself release authorization. C014 needs a repository/governance basis for any readiness transition and must not fabricate external runner/signing/browser evidence.

## Final verification phase — HLS-C016

Only after an accepted C014 merge, freeze that exact resulting `main` SHA as the prospective formal-release source. HLS-C016 then requires new successful v7 CI, v7 Candidate Package, Maintenance Security and Rust Security `push/main/exact-SHA` results for that frozen commit and reconfirms main has not moved.

HLS-C016 is evidence/handoff only. It must not merge post-freeze coordination files while the SHA is intended for formal release, because such a merge would itself move main and invalidate the frozen-SHA evidence. It does not tag, sign, dispatch or publish.

The formal workflow still requires the self-hosted Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, signing tool and trusted project certificate/private key, timestamp/network access, protected `v7-release` environment/approval, candidate-bound evidence/digest checks, and explicit operator publish choice.

## Dynamic replanning

- Any new reproducible source/security/governance contradiction becomes a separate P0/P1 task ahead of the readiness transition.
- A failed required workflow is investigated rather than waived. A cancelled run from an obsolete SHA after main moves is not a product regression, but it also cannot authorize a release.
- External runner/signing/browser/operator absence remains an external blocker; repository code must not simulate or weaken those trust boundaries.
- A participant truly returning after a valid >5-minute loss must redeclare and heartbeat. A loss decision disproved by an already-existing earlier heartbeat is corrected, not treated as a real departure/return cycle.
- A dependent task is interrupted immediately when a higher-priority audit/process correction becomes active; resume it only after the blocking task is durably accepted.

Every route change is mirrored in Issue #39, `docs/coordination/tasks.json`, `handoff.md`, `docs/manger.log`, and this plan when it changes the project route.
