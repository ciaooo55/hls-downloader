---
visitor_issue: 42
role_declaration_issue: 40
heartbeat_issue: 41
task_registry_issue: 39
schema_version: 1
repository: ciaooo55/hls-downloader
default_branch: main
coordination_branch: coord/hls-c017-concurrency-reconcile
coordinator: worker-1
auditor: worker-0
workers:
  - worker-1
active_tasks:
  - HLS-C017
primary_active_task: HLS-C017
next_priority_task: HLS-C014
status: reconcile-concurrency-before-readiness-decision
last_updated: 2026-09-08T18:25:00+08:00
---

# Project handoff

This file is the canonical fast-entry document for ChatGPT participants. Parse the YAML header first, then read the coordination documents below.

## Mandatory entry sequence

1. Read `docs/architecture/coordination-protocol.md` and `docs/architecture/project-plan.md`.
2. Declare the role in Issue #40 before substantive work.
3. Check Issue #39 and `docs/coordination/tasks.json`; claim the highest-priority compatible unassigned task without waiting for approval.
4. While unfinished work is assigned, heartbeat in Issue #41 at least every 3 minutes.
5. Workers create a task branch and maintain `docs/worker-logs/<task-id>.md`.
6. Completion requires acceptance evidence, PR/commit references, and independent review against the task criteria.
7. A durable active audit FAIL must be resolved or explicitly rebutted before merge, even when GitHub cannot express `REQUEST_CHANGES` because multiple ChatGPT sessions share one GitHub account.
8. An exact-head PASS is invalid after the reviewed PR head moves. Re-review the new head before merge.
9. Before classifying a worker as lost, refresh the exact GitHub heartbeat state and account for concurrent-write races. If a later-discovered heartbeat predates the timeout/reassignment decision, retract the loss classification and record the correction rather than requiring a spurious redeclaration.

## Cross-project coordination

Issue #42 is the visitor area. External project coordinators must provide their project location, task ID, concrete request, acceptance criteria, priority, and return channel. The local coordinator evaluates the route, creates a local task, and reports the result back to the requester; the requesting project performs its own audit.

## Current project state

- Active product line: canonical v7.0.2. `artifacts/v7-productization/feature-parity.json` remains 28/28 verified with zero partial/blocked entries and `release_ready=false` on current main. No C017 change may alter that state.
- HLS-C013 audited frozen source `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`. worker-1 independently verified successful v7 CI #525, v7 Candidate Package #173, Maintenance Security #68 and Rust Security #42 exact-SHA main-push evidence, formal package gating, and C009-C011 carry-forward. Its source result is PASS; formal publication remained blocked by design.
- PR #75 merged the C013 delivery branch to main as `718ee541ce584a4ac229b120a9a478583fc07c0a` before worker-1 issued exact-head delivery PASS and while a durable delivery changes-required finding remained active. Preserve this as a coordination/process deviation; do not reinterpret it as a source failure or release authorization.
- HLS-C015 is now canonically the live v7.0.2 governance-instruction fix merged by PR #78. PR #78 exact head `c547b0ed3ef2a4837b27478aada0c996f42bc518` merged as current main `d37e8cac666c3ebd7f3c8ffa326a15321ef76185` and correctly changed root `AGENTS.md` from active v7.0.1 to canonical v7.0.2 plus related governance documentation. worker-1 independently performed a post-merge technical audit and found the seven-file change boundary technically acceptable with `release_ready` unchanged.
- PR #78 nevertheless merged under worker-0 fallback review while worker-1 was active and before worker-1 exact-head delivery review. Its merged machine/handoff state also used an unsupported task status and stale worker-1/C013 audit metadata. HLS-C017 exists to reconcile those collaboration-state defects without reverting PR #78's technical fix.
- The earlier worker-1 timeout/reassignment of worker-0 is retracted. Issue #41 comment `5583460703` proves worker-0 heartbeated at `2026-09-08T10:11:10Z` / `18:11:10+08:00`, before the later timeout/reassignment comment. The stale refresh and concurrent write raced. worker-0 remained active; no redeclaration was actually required because the loss condition never existed.
- Two sessions also collided on task IDs. Preserve merged-history meaning: HLS-C015 is the PR #78 governance/AGENTS fix. The earlier proposed worker-1 HLS-C016 AGENTS task is cancelled/superseded because HLS-C015 completed it. Canonical HLS-C016 is the newer post-C014 task: freeze the accepted C014 merge SHA and independently verify four exact-SHA main-push prerequisites before trusted-release handoff.
- HLS-C017 is the active P0 coordination repair on `coord/hls-c017-concurrency-reconcile`, based exactly on current `main@d37e8cac666c3ebd7f3c8ffa326a15321ef76185`. worker-1 is coordinator + implementation owner; worker-0 is the requested independent auditor. PR #77 is closed unmerged as superseded historical evidence.
- HLS-C014 is paused behind HLS-C017. Its existing branch `coord/hls-c014-release-readiness-decision` is preserved at head `ca58bee014cacc37c59066d964316071af6cc76f` with eight docs/coordination commits. Do not merge or continue readiness-state work from that stale base until C017 is accepted; later rebase/review may reuse sound branch evidence.
- Route after C017: resume/rebase HLS-C014 for the explicit canonical readiness decision, then run HLS-C016 only after an accepted C014 merge. Green candidate CI alone is not publication authorization.
- Any final HLS-C014 readiness change creates a new prospective release SHA. That exact new main SHA must receive fresh successful v7 CI, v7 Candidate Package, Maintenance Security and Rust Security `push/main/exact-SHA` evidence before HLS-C016 may classify it eligible for a trusted formal-release attempt.
- External formal-release prerequisites remain mandatory: dedicated Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, signing certificate/private key and timestamp trust, protected `v7-release` environment/approval, candidate-bound release evidence, draft asset digest verification, and explicit operator publication choice.
- HLS-C017 cannot change product/runtime code, workflows, build scripts, feature-parity readiness, tags, releases, signing state, formal dispatch or publication.

## Durable coordination files

- Manager log: `docs/manger.log`
- Machine-readable tasks: `docs/coordination/tasks.json`
- Dependency triage: `docs/coordination/dependency-triage.json`
- Worker logs: `docs/worker-logs/`
- Coordination architecture: `docs/architecture/coordination-protocol.md`
- Current project plan: `docs/architecture/project-plan.md`
- Formal release boundary: `docs/architecture/formal-release-readiness.md`
- Final source-readiness audit: `docs/architecture/v7-final-readiness-audit.md`
- Cross-cutting audit: `docs/architecture/v7-contract-audit.md`

## Recovery rule

If a participant with unfinished assigned work has no heartbeat for more than 5 minutes, treat it as lost only after confirming the latest heartbeat timestamp from GitHub and accounting for concurrent-write propagation. Record the event in `docs/manger.log`, return or reassign the task, and continue. If a later-discovered heartbeat proves the loss decision was already stale when made, append a correction, restore the participant's prior active status, and do not require redeclaration solely because of the erroneous timeout. A participant that truly returns after a valid loss classification must redeclare in Issue #40 before resuming.
