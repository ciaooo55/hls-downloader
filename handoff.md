---
visitor_issue: 42
role_declaration_issue: 40
heartbeat_issue: 41
task_registry_issue: 39
schema_version: 1
repository: ciaooo55/hls-downloader
default_branch: main
coordination_branch: coord/hls-c015-c013-process-reconcile
coordinator: worker-1
auditor: worker-1
workers:
  - worker-0
active_tasks:
  - HLS-C015
primary_active_task: HLS-C015
next_priority_task: HLS-C014
status: reconcile-c013-premature-merge-before-readiness-governance
last_updated: 2026-09-08T18:05:00+08:00
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

- Active product line: v7.0.2. Canonical `artifacts/v7-productization/feature-parity.json` remains 28/28 verified with zero partial/blocked entries and `release_ready=false`.
- HLS-C012 merged through PR #73 as `1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`. That frozen SHA was the source audited by HLS-C013.
- The C013 frozen SHA had successful `v7 CI` #525, `v7 Candidate Package` #173, `Maintenance Security` #68 and `Rust Security` #42 **push/main/exact-SHA** workflow evidence. `worker-1` independently revalidated the frozen SHA, candidate artifact binding, formal package gates and carried-forward C009-C011 hardening and issued a source-level PASS.
- PR #75 nevertheless merged C013 to `main` as `718ee541ce584a4ac229b120a9a478583fc07c0a` from head `d6221b55d28e1d9c596840637db9e1e4ff0287f9` before `worker-1` issued an exact-head delivery PASS and while a durable delivery changes-required finding remained active. PR #75 also described the audit as fallback/no-independent-auditor even though `worker-1` had already re-declared and heartbeated. This is a collaboration/process deviation, not a new product-source defect.
- HLS-C015 is now the active P0 correction task on `coord/hls-c015-c013-process-reconcile`, based exactly on current `main@718ee541ce584a4ac229b120a9a478583fc07c0a`. Its purpose is to restore truthful machine/handoff/manager-log state and the independent audit chain without changing product or release behavior.
- `worker-1` is the current coordinator and independent auditor. `worker-0` is the one active ordinary worker and owns the required HLS-C015 manager-log continuation. Helper work by `worker-1` may use child/narrow commits but does not turn the correction into self-approval.
- HLS-C014 readiness governance is paused behind HLS-C015. Its branch `coord/hls-c014-release-readiness-decision` was created at `718ee541...` and had no task commit when interrupted, so no implementation work is lost.
- Because PR #75 moved `main`, the four successful workflow runs for old frozen SHA `1c93cffe...` remain valid C013 source-audit evidence but can never authorize a formal release of the newer main. Any eventual readiness-authorized final main SHA must receive a fresh four-workflow exact-SHA set.
- C013/C015/C014 are genuine project-closeout work. Do not manufacture filler tasks solely to satisfy the normal 2× backlog target.
- External formal-release prerequisites remain mandatory: dedicated Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, signing certificate/private key and timestamp trust, protected `v7-release` environment/approval, and explicit operator publish choice after digest verification.
- HLS-C015 cannot set `release_ready=true`, change product/runtime/workflows/build scripts, create a tag/release, sign artifacts, dispatch a formal release, or publish.
- Process correction from HLS-C003 remains durable: PR #64 previously merged while worker-1 had an unresolved factual FAIL. HLS-C004/PR #65 repaired that stale state. PR #75 is now explicitly treated as the same class of process deviation and must be reconciled before C014 resumes.

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

If a participant with unfinished assigned work has no heartbeat for more than 5 minutes, treat it as lost only after confirming the heartbeat timestamp from GitHub rather than from a truncated/paginated view. The coordinator records the event in `docs/manger.log`, returns or reassigns the task in the registry, and continues work. A returning participant must declare its role again in Issue #40 before resuming.
