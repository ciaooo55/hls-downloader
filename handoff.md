---
visitor_issue: 42
role_declaration_issue: 40
heartbeat_issue: 41
task_registry_issue: 39
schema_version: 1
repository: ciaooo55/hls-downloader
default_branch: main
coordination_branch: coord/hls-c014-release-readiness-decision
coordinator: worker-0
auditor: worker-0
workers:
  - worker-0
active_tasks:
  - HLS-C014
primary_active_task: HLS-C014
next_priority_task: HLS-C016
status: canonical-readiness-decision-in-progress
last_updated: 2026-09-08T18:21:00+08:00
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
9. When the independent worker is lost, use the documented fallback only after confirming the last GitHub heartbeat timestamp; record the limitation instead of pretending that self-review is independent.

## Cross-project coordination

Issue #42 is the visitor area. External project coordinators must provide their project location, task ID, concrete request, acceptance criteria, priority, and return channel. The local coordinator evaluates the route, creates a local task, and reports the result back to the requester; the requesting project performs its own audit.

## Current project state

- Active product line: v7.0.2. Canonical `artifacts/v7-productization/feature-parity.json` is 28/28 verified with zero partial/blocked entries. At C014 start it still carries `release_ready=false` and `audit_state=v7_0_2_iteration_in_progress`.
- HLS-C013 / PR #75 completed the final frozen-source audit and concluded that the source/CI contract was clean while formal publication remained intentionally blocked by the canonical readiness policy and external trusted-release prerequisites.
- HLS-C015 / PR #78 is complete. Its first head `05f84711e23eb8299600b5fdc751055b3583bf19` was rejected because the live project plan was stale; the corrected exact head `c547b0ed3ef2a4837b27478aada0c996f42bc518` passed fallback review and merged as `d37e8cac666c3ebd7f3c8ffa326a15321ef76185`.
- C015 reconciled live governance only: root `AGENTS.md` now names v7.0.2 as active, canonical feature-parity metadata as version/readiness truth, and explicitly requires fully verified parity + a separately reviewed `release_ready=true` decision + visual/performance/installer/rollback gates. Candidate CI is not publication authorization.
- HLS-C014 is active on `coord/hls-c014-release-readiness-decision`, fast-forwarded to post-C015 `main@d37e8cac666c3ebd7f3c8ffa326a15321ef76185` before task commits.
- Repository history establishes the readiness-state precedent: the v7.0.1 release-ready state used `release_ready=true` with `audit_state=v7_0_1_release_specialties_verified`; the automated v7.0.2 iteration-start commit reset them together to `false` and `v7_0_2_iteration_in_progress`. C014 therefore treats readiness as repository-governance permission to enter the existing formal gate, not proof that external release prerequisites already passed.
- On the post-C015 main baseline, v7 CI #527, Maintenance Security #70, and Rust Security #44 completed successfully. v7 Candidate Package #175 remained in progress at the last refresh. C014 has not counted an in-progress run as success and has not changed canonical readiness metadata yet.
- If the decision conditions remain satisfied after the final baseline refresh, C014's proposed narrow metadata transition is `release_ready: false -> true` and `audit_state: v7_0_2_iteration_in_progress -> v7_0_2_release_specialties_verified`. No feature item, version, workflow, runtime or formal gate is part of that transition.
- HLS-C016 is the next P0 task and preserves the two-unfinished-task backlog for one active worker. After an accepted C014 merge, C016 must freeze that exact new main SHA and require four fresh successful `push/main/exact-SHA` prerequisite workflows. Predecessor-SHA results do not authorize formal release.
- C016 must not merge post-freeze evidence/coordination commits into `main` while the frozen SHA is intended for release, because any such merge would create a new prospective release SHA and invalidate the old exact-SHA set. Evidence may remain in Issue #39 / Issue #41 and on an unmerged evidence branch during the freeze.
- External formal-release prerequisites remain mandatory: dedicated Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, signing certificate/private key and timestamp trust, protected `v7-release` environment/approval, candidate-bound visual/performance/MSI/rollback evidence, draft asset digest verification, and explicit operator publish choice.
- No current task authorizes tagging, signing, formal-release dispatch, release creation or publication.
- Process correction from HLS-C003 remains durable: PR #64 merged while worker-1 had an unresolved factual FAIL. HLS-C004/PR #65 repaired it. Future merges must resolve or explicitly rebut every durable active FAIL before merge.
- `worker-1` remains inactive after heartbeat timeout. A returning participant must redeclare and heartbeat before resuming work. Current active worker count is 1; unfinished HLS-C014 + HLS-C016 preserves the minimum two-task backlog.

## Durable coordination files

- Manager log: `docs/manger.log`
- Machine-readable tasks: `docs/coordination/tasks.json`
- Dependency triage: `docs/coordination/dependency-triage.json`
- Worker logs: `docs/worker-logs/`
- Coordination architecture: `docs/architecture/coordination-protocol.md`
- Current project plan: `docs/architecture/project-plan.md`
- Formal release boundary: `docs/architecture/formal-release-readiness.md`
- Final source-readiness audit: `docs/architecture/v7-final-readiness-audit.md`
- Canonical readiness decision: `docs/architecture/v7-release-readiness-decision.md`
- Cross-cutting audit: `docs/architecture/v7-contract-audit.md`

## Recovery rule

If a participant with unfinished assigned work has no heartbeat for more than 5 minutes, treat it as lost only after confirming the heartbeat timestamp from GitHub rather than from a truncated/paginated view. The coordinator records the event in `docs/manger.log`, returns or reassigns the task in the registry, and continues work. A returning participant must declare its role again in Issue #40 before resuming.
